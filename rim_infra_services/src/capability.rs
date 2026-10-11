//! Default `CapabilityService` implementation: a strict default-deny gate backed by the
//! `capability_registry` table plus a workspace-membership check, and for some capabilities a
//! minimum role in that workspace.
//!
//! See [`rim_services::capability::service::CapabilityService`] for the
//! gate contract. This module also exports [`builtin_capabilities`] for the
//! startup seeding path called from `rim_server::app`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use rim_core::capability::repository::CapabilityRegistryRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;

use rim_services::capability::service::CapabilityService;
use rim_services::{Actor, ServiceError, ServiceResult};

/// A builtin capability. Every entry of [`builtin_capabilities`] is inserted into
/// `capability_registry` at startup, before the HTTP listener binds, so the first request
/// never races the seeding.
///
/// Adding a builtin cap means adding a row to [`builtin_capabilities`]; the next startup
/// seeds it. A forgotten entry fails closed: the registry lookup finds no row and the gate
/// returns `PermissionDenied`.
pub struct BuiltinCapabilitySpec {
    pub id: &'static str,
    /// When `true` the capability is self-scoped: it does not require a
    /// workspace context and `check()` accepts `workspace_id = None`.
    ///
    /// When `false` the capability is workspace-scoped: `check()` rejects
    /// `workspace_id = None` with `PermissionDenied` because there is no
    /// meaningful membership to validate without a workspace. Callers must
    /// supply a workspace context for all workspace-scoped caps.
    pub is_self_scoped: bool,
    /// When `true` the caller must be an Owner or Moderator of the workspace; membership alone
    /// is not enough. Lives only here, not in `capability_registry`: the gate reads it from this
    /// table by key.
    pub requires_moderator: bool,
}

pub fn builtin_capabilities() -> &'static [BuiltinCapabilitySpec] {
    &[
        BuiltinCapabilitySpec {
            id: "chat.read",
            is_self_scoped: false,
            requires_moderator: false,
        },
        BuiltinCapabilitySpec {
            id: "chat.write",
            is_self_scoped: false,
            requires_moderator: false,
        },
        BuiltinCapabilitySpec {
            id: "action.invoke",
            is_self_scoped: false,
            requires_moderator: false,
        },
        BuiltinCapabilitySpec {
            id: "event.emit",
            is_self_scoped: false,
            requires_moderator: false,
        },
        BuiltinCapabilitySpec {
            id: "audit.read",
            is_self_scoped: false,
            requires_moderator: true,
        },
        BuiltinCapabilitySpec {
            id: "audit.read.self",
            is_self_scoped: true,
            requires_moderator: false,
        },
        BuiltinCapabilitySpec {
            id: "admin.manage_members",
            is_self_scoped: false,
            requires_moderator: false,
        },
    ]
}

/// Whether the builtin capability `key` requires an Owner or Moderator role. `false` for keys
/// that are not builtin.
fn requires_moderator(key: &str) -> bool {
    builtin_capabilities()
        .iter()
        .any(|spec| spec.id == key && spec.requires_moderator)
}

/// Whether a caller holding `role` in the workspace (`None`: not a member) passes the
/// workspace leg for capability `key`.
fn role_satisfies(key: &str, role: Option<ServerMemberRole>) -> bool {
    match role {
        None => false,
        Some(role) => !requires_moderator(key) || role.is_moderator(),
    }
}

#[derive(Clone)]
pub struct CapabilityRepos {
    pub capability_registry_repo: Arc<dyn CapabilityRegistryRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

#[derive(Clone)]
pub struct DefaultCapabilityService {
    repos: CapabilityRepos,
}

impl DefaultCapabilityService {
    pub fn new(repos: CapabilityRepos) -> Self {
        Self { repos }
    }

    /// The actor's role in `workspace_id`, or `None` when they are not a member.
    async fn member_role(
        &self,
        actor: &Actor,
        workspace_id: ServerId,
    ) -> ServiceResult<Option<ServerMemberRole>> {
        let member = self
            .repos
            .server_member_repo
            .get_member(workspace_id, actor.user_id)
            .await
            .map_err(ServiceError::from)?;
        Ok(member.map(|m| m.member_role))
    }
}

#[async_trait]
impl CapabilityService for DefaultCapabilityService {
    async fn check(
        &self,
        actor: &Actor,
        workspace_id: Option<ServerId>,
        capability: &str,
    ) -> ServiceResult<()> {
        // Step 1: empty / whitespace input is a programming error AND a
        // potential capability-bypass primitive (a future contributor doing
        // `required_capabilities: vec![String::new()]` would otherwise pass
        // the gate with no real check). Reject.
        let key = capability.trim();
        if key.is_empty() {
            return Err(ServiceError::Validation(
                "capability key must be non-empty".to_string(),
            ));
        }

        // Step 2: registry lookup — single `get()` call returns the full row
        // (including `is_self_scoped` and `retired_at`) so we can enforce
        // both strict default-deny and workspace scope in one
        // round-trip instead of two. `get()` returns `None` for unregistered
        // keys; a row with `retired_at IS NOT NULL` is treated as unregistered.
        let row = self.repos.capability_registry_repo.get(key).await?;

        let reg = match row {
            Some(r) if r.retired_at.is_none() => r,
            _ => {
                tracing::warn!(
                    target: "audit",
                    event = "capability_check_unknown_key",
                    user_id = %actor.user_id,
                    workspace_id = ?workspace_id,
                    capability = key,
                    "capability check denied — key not registered (strict default-deny)"
                );
                return Err(ServiceError::PermissionDenied);
            }
        };

        // Step 2b: workspace-scope enforcement.
        // DB-stored `is_self_scoped` is the authoritative source — not the
        // static in-memory set. Workspace-scoped caps require a workspace_id;
        // passing None would skip the membership check and is a security
        // defect. Self-scoped caps (e.g. `audit.read.self`) explicitly permit
        // None since membership is not meaningful for identity-only operations.
        if workspace_id.is_none() && !reg.is_self_scoped {
            tracing::warn!(
                target: "audit",
                event = "capability_check_missing_workspace",
                user_id = %actor.user_id,
                capability = key,
                "capability check denied — workspace-scoped capability called \
                 without a workspace_id (membership check would be bypassed)"
            );
            return Err(ServiceError::PermissionDenied);
        }

        // Step 3: workspace leg. Without a workspace only self-scoped caps get here and there is
        // nothing more to check. With one, the actor must be a member, and an Owner or
        // Moderator for caps that require it.
        let Some(workspace_id) = workspace_id else {
            return Ok(());
        };
        let role = self.member_role(actor, workspace_id).await?;
        if role.is_none() {
            tracing::warn!(
                target: "audit",
                event = "capability_check_not_a_member",
                user_id = %actor.user_id,
                workspace_id = %workspace_id,
                capability = key,
                "capability check denied — actor is not a member of workspace"
            );
            return Err(ServiceError::PermissionDenied);
        }
        if !role_satisfies(key, role) {
            tracing::warn!(
                target: "audit",
                event = "capability_check_role_too_low",
                user_id = %actor.user_id,
                workspace_id = %workspace_id,
                capability = key,
                role = ?role,
                "capability check denied — capability requires an owner or moderator"
            );
            return Err(ServiceError::PermissionDenied);
        }

        Ok(())
    }

    async fn check_many(
        &self,
        actor: &Actor,
        workspace_id: Option<ServerId>,
        capabilities: &[String],
    ) -> ServiceResult<HashMap<String, bool>> {
        // Look up the role ONCE. The hot path here is "filter the AI
        // tool surface to caps the user can call", which can be 50+ caps —
        // re-querying server_member on every loop iteration would be a
        // pointless N+1.
        let role = match workspace_id {
            Some(workspace_id) => self.member_role(actor, workspace_id).await?,
            None => None,
        };

        let mut out = HashMap::with_capacity(capabilities.len());
        for cap in capabilities {
            let key = cap.trim();
            if key.is_empty() {
                out.insert(cap.clone(), false);
                continue;
            }
            // Use `get()` to obtain the full registry row: we need
            // the DB-stored `is_self_scoped` flag to enforce workspace-context
            // requirements. Workspace-scoped cap with no workspace_id → deny
            // without reaching the membership leg (same enforcement as
            // `check()`). Self-scoped caps pass through even with None.
            let row = self.repos.capability_registry_repo.get(key).await?;

            let reg = match row {
                Some(r) if r.retired_at.is_none() => r,
                _ => {
                    // Unknown or retired — deny.
                    out.insert(cap.clone(), false);
                    continue;
                }
            };

            // Workspace-scope check using the DB-stored flag.
            if workspace_id.is_none() && !reg.is_self_scoped {
                out.insert(cap.clone(), false);
                continue;
            }

            let allowed = workspace_id.is_none() || role_satisfies(key, role);
            out.insert(cap.clone(), allowed);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use mockall::predicate::*;
    use rim_core::capability::model::RegisteredCapability;
    use rim_core::capability::repository::MockCapabilityRegistryRepository;
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{ServerMember, ServerMemberId};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use uuid::Uuid;

    fn actor() -> Actor {
        Actor::new(UserId::from(Uuid::new_v4()))
    }

    /// A member repo whose `get_member` returns `role` (`None`: not a member), called at most
    /// `times` times.
    fn members_with(role: Option<ServerMemberRole>, times: usize) -> MockServerMemberRepository {
        let mut members = MockServerMemberRepository::new();
        members
            .expect_get_member()
            .times(times)
            .returning(move |server_id, user_id| {
                Ok(role.map(|member_role| ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role,
                    joined_at: chrono::Utc::now(),
                }))
            });
        members
    }

    fn workspace() -> ServerId {
        ServerId::from(Uuid::new_v4())
    }

    fn build(
        registry: MockCapabilityRegistryRepository,
        members: MockServerMemberRepository,
    ) -> DefaultCapabilityService {
        DefaultCapabilityService::new(CapabilityRepos {
            capability_registry_repo: Arc::new(registry),
            server_member_repo: Arc::new(members),
        })
    }

    /// Build an active (non-retired) `RegisteredCapability` for use in mock
    /// `get()` expectations. `is_self_scoped` sets the row's scope flag.
    fn active_cap(key: &str, is_self_scoped: bool) -> RegisteredCapability {
        RegisteredCapability {
            capability_key: key.to_string(),
            registered_at: chrono::Utc::now(),
            retired_at: None,
            is_self_scoped,
        }
    }

    #[tokio::test]
    async fn check_empty_capability_rejects_with_validation() {
        let svc = build(
            MockCapabilityRegistryRepository::new(),
            MockServerMemberRepository::new(),
        );
        let err = svc
            .check(&actor(), Some(workspace()), "")
            .await
            .unwrap_err();
        match err {
            ServiceError::Validation(_) => (),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn check_whitespace_capability_rejects_with_validation() {
        let svc = build(
            MockCapabilityRegistryRepository::new(),
            MockServerMemberRepository::new(),
        );
        let err = svc
            .check(&actor(), Some(workspace()), "   ")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    /// An unknown cap denies even when the actor
    /// is a workspace member". `get()` returns `None` → `PermissionDenied`.
    /// If this test ever passes when `get` returns `None`, the strict
    /// default-deny has regressed to no-op-allow.
    #[tokio::test]
    async fn check_unknown_capability_denies_strict_default() {
        let mut registry = MockCapabilityRegistryRepository::new();
        registry
            .expect_get()
            .with(eq("nonexistent.cap"))
            .times(1)
            .returning(|_| Ok(None));
        // Membership repo MUST NOT be queried — the registry leg fails
        // first. mockall enforces zero calls implicitly on an empty
        // expectation list.
        let members = MockServerMemberRepository::new();

        let svc = build(registry, members);
        let err = svc
            .check(&actor(), Some(workspace()), "nonexistent.cap")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    /// A retired cap (retired_at IS NOT NULL) is treated
    /// the same as unregistered: strict default-deny.
    #[tokio::test]
    async fn check_retired_capability_denies_strict_default() {
        let mut registry = MockCapabilityRegistryRepository::new();
        registry
            .expect_get()
            .with(eq("chat.write"))
            .times(1)
            .returning(|_| {
                let mut cap = active_cap("chat.write", false);
                cap.retired_at = Some(chrono::Utc::now());
                Ok(Some(cap))
            });
        let members = MockServerMemberRepository::new();

        let svc = build(registry, members);
        let err = svc
            .check(&actor(), Some(workspace()), "chat.write")
            .await
            .unwrap_err();
        assert!(
            matches!(err, ServiceError::PermissionDenied),
            "retired cap must deny like an unregistered cap, got {err:?}"
        );
    }

    #[tokio::test]
    async fn check_known_capability_with_member_actor_succeeds() {
        let mut registry = MockCapabilityRegistryRepository::new();
        registry
            .expect_get()
            .with(eq("chat.write"))
            .times(1)
            .returning(|_| Ok(Some(active_cap("chat.write", false))));
        let members = members_with(Some(ServerMemberRole::Member), 1);

        let svc = build(registry, members);
        svc.check(&actor(), Some(workspace()), "chat.write")
            .await
            .expect("known cap + member should pass");
    }

    #[tokio::test]
    async fn check_known_capability_with_non_member_actor_denies() {
        let mut registry = MockCapabilityRegistryRepository::new();
        registry
            .expect_get()
            .returning(|_| Ok(Some(active_cap("chat.write", false))));
        let members = members_with(None, 1);

        let svc = build(registry, members);
        let err = svc
            .check(&actor(), Some(workspace()), "chat.write")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    /// A self-scoped cap with `workspace_id = None` passes.
    /// `audit.read.self` is the canonical self-scoped cap: the actor reads
    /// their own audit history without needing a workspace context.
    #[tokio::test]
    async fn check_self_scoped_cap_without_workspace_succeeds() {
        let mut registry = MockCapabilityRegistryRepository::new();
        registry
            .expect_get()
            .with(eq("audit.read.self"))
            .returning(|_| Ok(Some(active_cap("audit.read.self", true))));
        // membership repo must NOT be queried for self-scoped caps with None
        let members = MockServerMemberRepository::new();

        let svc = build(registry, members);
        svc.check(&actor(), None, "audit.read.self")
            .await
            .expect("self-scoped cap with no workspace must pass");
    }

    /// A workspace-scoped cap with `workspace_id = None` is `PermissionDenied`;
    /// without a workspace there is no membership to check.
    #[tokio::test]
    async fn check_workspace_scoped_cap_without_workspace_denies() {
        let mut registry = MockCapabilityRegistryRepository::new();
        registry
            .expect_get()
            .with(eq("chat.write"))
            .returning(|_| Ok(Some(active_cap("chat.write", false))));
        // membership repo must NOT be queried — rejection happens before it
        let members = MockServerMemberRepository::new();

        let svc = build(registry, members);
        let err = svc.check(&actor(), None, "chat.write").await.unwrap_err();
        assert!(
            matches!(err, ServiceError::PermissionDenied),
            "workspace-scoped cap without workspace_id must be PermissionDenied, got {err:?}"
        );
    }

    /// `check_many` with `workspace_id = None` denies
    /// workspace-scoped caps but still allow self-scoped ones.
    #[tokio::test]
    async fn check_many_workspace_scoped_cap_without_workspace_returns_false() {
        let mut registry = MockCapabilityRegistryRepository::new();
        // audit.read.self is self-scoped → get() is called, returns is_self_scoped=true
        registry
            .expect_get()
            .with(eq("audit.read.self"))
            .returning(|_| Ok(Some(active_cap("audit.read.self", true))));
        // chat.write is workspace-scoped → get() is called, returns is_self_scoped=false,
        // then denied before reaching the membership leg
        registry
            .expect_get()
            .with(eq("chat.write"))
            .returning(|_| Ok(Some(active_cap("chat.write", false))));
        // workspace_id is None → the member repo is never queried
        let members = MockServerMemberRepository::new();

        let svc = build(registry, members);
        let map = svc
            .check_many(
                &actor(),
                None,
                &["chat.write".to_string(), "audit.read.self".to_string()],
            )
            .await
            .expect("check_many must not error");
        assert_eq!(
            map.get("chat.write"),
            Some(&false),
            "workspace-scoped cap without workspace_id must be false"
        );
        assert_eq!(
            map.get("audit.read.self"),
            Some(&true),
            "self-scoped cap must still pass without workspace_id"
        );
    }

    #[tokio::test]
    async fn check_many_returns_per_cap_bool_map() {
        let mut registry = MockCapabilityRegistryRepository::new();
        registry
            .expect_get()
            .with(eq("chat.write"))
            .returning(|_| Ok(Some(active_cap("chat.write", false))));
        registry
            .expect_get()
            .with(eq("plugin.unknown.x"))
            .returning(|_| Ok(None));
        // Crucial — bulk path looks up the role exactly once even
        // though we ask for two caps. Catches an N+1 regression.
        let members = members_with(Some(ServerMemberRole::Member), 1);

        let svc = build(registry, members);
        let map = svc
            .check_many(
                &actor(),
                Some(workspace()),
                &["chat.write".to_string(), "plugin.unknown.x".to_string()],
            )
            .await
            .expect("check_many should succeed");
        assert_eq!(map.get("chat.write"), Some(&true));
        assert_eq!(map.get("plugin.unknown.x"), Some(&false));
    }

    /// Every `required_capabilities` entry on every
    /// builtin command MUST be in the seeded capability registry, otherwise
    /// the strict default-deny gate silently bricks that command at runtime.
    ///
    /// This is a structural CI guard: a contributor adding `vec!["foo.bar"]`
    /// to a builtin's `required_capabilities` without also adding `foo.bar`
    /// to `builtin_capabilities()` will fail this test. The fix is to add
    /// the cap to the seed list (`capability.rs::builtin_capabilities`),
    /// not to special-case the command.
    #[test]
    fn every_builtin_required_capability_is_seeded_in_registry() {
        let seeded: HashSet<&str> = builtin_capabilities().iter().map(|c| c.id).collect();

        let mut missing: Vec<(String, String)> = Vec::new();
        for cmd in crate::command::registry::builtin_commands() {
            for cap in &cmd.required_capabilities {
                if !seeded.contains(cap.as_str()) {
                    missing.push((cmd.id.clone(), cap.clone()));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "Builtin command(s) declare required_capabilities that are NOT in \
             `builtin_capabilities()` — the strict default-deny gate will brick \
             them at runtime. Either add the cap to the seed list, or remove the \
             requirement from the command. Offenders: {missing:?}"
        );
    }

    /// Locks the scope classification of the seeded builtin caps: `audit.read.self` is
    /// self-scoped; `chat.read`, `chat.write`, `audit.read` and `admin.manage_members` are
    /// workspace-scoped and are denied without a workspace context.
    #[test]
    fn all_builtin_caps_have_explicit_is_self_scoped_flag() {
        let workspace_scoped: Vec<&str> = builtin_capabilities()
            .iter()
            .filter(|c| !c.is_self_scoped)
            .map(|c| c.id)
            .collect();
        let self_scoped: Vec<&str> = builtin_capabilities()
            .iter()
            .filter(|c| c.is_self_scoped)
            .map(|c| c.id)
            .collect();

        assert!(
            self_scoped.contains(&"audit.read.self"),
            "audit.read.self must be classified as self-scoped"
        );
        assert!(
            workspace_scoped.contains(&"chat.read"),
            "chat.read must be workspace-scoped"
        );
        assert!(
            workspace_scoped.contains(&"chat.write"),
            "chat.write must be workspace-scoped"
        );
        assert!(
            workspace_scoped.contains(&"audit.read"),
            "audit.read must be workspace-scoped"
        );
        assert!(
            workspace_scoped.contains(&"admin.manage_members"),
            "admin.manage_members must be workspace-scoped"
        );
    }

    #[tokio::test]
    async fn check_many_empty_string_in_input_returns_false_not_error() {
        // A buggy caller passing an empty cap shouldn't poison the whole
        // batch. Bulk path keeps the input/output shape symmetric.
        let registry = MockCapabilityRegistryRepository::new();
        let members = members_with(Some(ServerMemberRole::Member), 1);

        let svc = build(registry, members);
        let map = svc
            .check_many(&actor(), Some(workspace()), &[String::new()])
            .await
            .expect("empty-string entry should not propagate as error");
        assert_eq!(map.get(""), Some(&false));
    }
    /// `check("audit.read", Some(W))` for a caller holding `role` in W (`None`: not a member).
    async fn check_audit_read(role: Option<ServerMemberRole>) -> ServiceResult<()> {
        let mut registry = MockCapabilityRegistryRepository::new();
        registry
            .expect_get()
            .with(eq("audit.read"))
            .times(1)
            .returning(|_| Ok(Some(active_cap("audit.read", false))));
        let svc = build(registry, members_with(role, 1));
        svc.check(&actor(), Some(workspace()), "audit.read").await
    }

    #[tokio::test]
    async fn check_audit_read_owner_passes() {
        check_audit_read(Some(ServerMemberRole::Owner))
            .await
            .expect("owner must hold audit.read");
    }

    #[tokio::test]
    async fn check_audit_read_moderator_passes() {
        check_audit_read(Some(ServerMemberRole::Moderator))
            .await
            .expect("moderator must hold audit.read");
    }

    #[tokio::test]
    async fn check_audit_read_member_denied() {
        let err = check_audit_read(Some(ServerMemberRole::Member))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied), "{err:?}");
    }

    #[tokio::test]
    async fn check_audit_read_guest_denied() {
        let err = check_audit_read(Some(ServerMemberRole::Guest))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied), "{err:?}");
    }

    #[tokio::test]
    async fn check_audit_read_non_member_denied() {
        let err = check_audit_read(None).await.unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied), "{err:?}");
    }

    /// The role requirement is per capability: a guest still holds the membership-only
    /// `chat.read`.
    #[tokio::test]
    async fn check_membership_only_cap_passes_for_guest() {
        let mut registry = MockCapabilityRegistryRepository::new();
        registry
            .expect_get()
            .with(eq("chat.read"))
            .returning(|_| Ok(Some(active_cap("chat.read", false))));
        let svc = build(registry, members_with(Some(ServerMemberRole::Guest), 1));
        svc.check(&actor(), Some(workspace()), "chat.read")
            .await
            .expect("guest holds membership-only caps");
    }

    /// `audit.read.self` carries no role requirement: a guest passes it inside a workspace and
    /// without one.
    #[tokio::test]
    async fn check_audit_read_self_unaffected_by_role() {
        let mut registry = MockCapabilityRegistryRepository::new();
        registry
            .expect_get()
            .with(eq("audit.read.self"))
            .returning(|_| Ok(Some(active_cap("audit.read.self", true))));
        let svc = build(registry, members_with(Some(ServerMemberRole::Guest), 1));
        svc.check(&actor(), Some(workspace()), "audit.read.self")
            .await
            .expect("guest holds audit.read.self in a workspace");
        svc.check(&actor(), None, "audit.read.self")
            .await
            .expect("audit.read.self needs no workspace");
    }

    /// `check_many` applies the role requirement per cap from the single role lookup.
    #[tokio::test]
    async fn check_many_applies_role_requirement_per_cap() {
        let caps = ["audit.read", "audit.read.self", "chat.read"].map(String::from);
        for (role, audit_read) in [
            (ServerMemberRole::Owner, true),
            (ServerMemberRole::Moderator, true),
            (ServerMemberRole::Member, false),
            (ServerMemberRole::Guest, false),
        ] {
            let mut registry = MockCapabilityRegistryRepository::new();
            registry
                .expect_get()
                .returning(|key| Ok(Some(active_cap(key, key == "audit.read.self"))));
            let svc = build(registry, members_with(Some(role), 1));
            let map = svc
                .check_many(&actor(), Some(workspace()), &caps)
                .await
                .expect("check_many must not error");
            assert_eq!(map.get("audit.read"), Some(&audit_read), "{role:?}");
            assert_eq!(map.get("audit.read.self"), Some(&true), "{role:?}");
            assert_eq!(map.get("chat.read"), Some(&true), "{role:?}");
        }
    }

    /// Only `audit.read` requires an Owner or Moderator among the builtin caps.
    #[test]
    fn only_audit_read_requires_moderator() {
        let gated: Vec<&str> = builtin_capabilities()
            .iter()
            .filter(|c| c.requires_moderator)
            .map(|c| c.id)
            .collect();
        assert_eq!(gated, vec!["audit.read"]);
    }
}
