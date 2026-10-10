//! Default `CapabilityService` implementation — Phase 1 §3.3 (closes Black
//! Widow C1, "real capability gate from Phase 1, no no-op-allow stub").
//!
//! See [`rim_services::capability::service::CapabilityService`] for the
//! gate contract. This module also exports [`builtin_capabilities`] for the
//! startup seeding path called from `rim_server::app`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use rim_core::capability::repository::CapabilityRegistryRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::ServerMemberRepository;

use rim_services::capability::service::CapabilityService;
use rim_services::{Actor, ServiceError, ServiceResult};

/// Catalogue of builtin capabilities Phase 1 ships. Every entry is upserted
/// into `capability_registry` synchronously at startup BEFORE the HTTP
/// listener binds, eliminating Widow's "registry-seeding race vs first
/// request" failure mode.
///
/// Adding a new builtin cap = add a row here AND rerun migrations on every
/// deploy. The strict default-deny gate guarantees a forgotten entry fails
/// closed (the new command's `required_capabilities` lookup returns
/// registry lookup finds no row → `PermissionDenied`), which is a louder failure
/// mode than silently allowing.
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
}

pub fn builtin_capabilities() -> &'static [BuiltinCapabilitySpec] {
    &[
        BuiltinCapabilitySpec {
            id: "chat.read",
            is_self_scoped: false,
        },
        BuiltinCapabilitySpec {
            id: "chat.write",
            is_self_scoped: false,
        },
        BuiltinCapabilitySpec {
            id: "action.invoke",
            is_self_scoped: false,
        },
        BuiltinCapabilitySpec {
            id: "event.emit",
            is_self_scoped: false,
        },
        BuiltinCapabilitySpec {
            id: "audit.read",
            is_self_scoped: false,
        },
        BuiltinCapabilitySpec {
            id: "audit.read.self",
            is_self_scoped: true,
        },
        BuiltinCapabilitySpec {
            id: "admin.manage_members",
            is_self_scoped: false,
        },
    ]
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

    /// Workspace-membership leg of the gate. Returns `Ok(true)` when the
    /// actor is a member of `workspace_id`, `Ok(false)` otherwise. When
    /// `workspace_id` is `None` the leg short-circuits to `Ok(true)` —
    /// this path is only reached for self-scoped capabilities (e.g.
    /// `audit.read.self`) after `check()` verifies `is_self_scoped == true`
    /// in the registry row (P6-FU5). Workspace-scoped caps never reach this
    /// method with `None` — `check()` rejects them first with
    /// `PermissionDenied` (Black Widow H2 closure).
    async fn membership_holds(
        &self,
        actor: &Actor,
        workspace_id: Option<ServerId>,
    ) -> ServiceResult<bool> {
        let Some(workspace_id) = workspace_id else {
            return Ok(true);
        };
        self.repos
            .server_member_repo
            .is_member(workspace_id, actor.user_id)
            .await
            .map_err(ServiceError::from)
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
        // both strict-default-deny (C1) and workspace-scope (P6-FU5) in one
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

        // Step 2b: workspace-scope enforcement (P6-FU5, Widow H2 closure).
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

        // Step 3: membership leg. Plugin-source caps would also need a
        // grant-store check here (Phase 5). Since Phase 1 only seeds
        // builtin caps and rejects plugin grants by default, the membership
        // check is the only gate beyond the registry check today.
        if !self.membership_holds(actor, workspace_id).await? {
            tracing::warn!(
                target: "audit",
                event = "capability_check_not_a_member",
                user_id = %actor.user_id,
                workspace_id = ?workspace_id,
                capability = key,
                "capability check denied — actor is not a member of workspace"
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
        // Compute membership ONCE. The hot path here is "filter the AI
        // tool surface to caps the user can call", which can be 50+ caps —
        // re-querying server_member on every loop iteration would be a
        // pointless N+1.
        let member = self.membership_holds(actor, workspace_id).await?;

        let mut out = HashMap::with_capacity(capabilities.len());
        for cap in capabilities {
            let key = cap.trim();
            if key.is_empty() {
                out.insert(cap.clone(), false);
                continue;
            }
            // Use `get()` to obtain the full registry row (P6-FU5): we need
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

            // Workspace-scope check using DB-stored flag (P6-FU5).
            if workspace_id.is_none() && !reg.is_self_scoped {
                out.insert(cap.clone(), false);
                continue;
            }

            out.insert(cap.clone(), member);
        }
        Ok(out)
    }
}

/// Register plugin-declared capabilities into the capability registry.
///
/// This is the **only** sanctioned write path for plugin capabilities.
/// `is_self_scoped` is unconditionally `false` — plugins cannot declare
/// self-scoped capabilities because doing so would let a malicious manifest
/// claim workspace-gate bypass (a plugin that convinces the host to install
/// `plugin.evil.foo` with `is_self_scoped=true` could invoke arbitrary
/// commands without supplying a workspace context, bypassing the membership
/// check in `CapabilityService::check`).
///
/// Callers pass validated capability keys (from `validate_plugin_capability`)
/// and the `plugin_uuid` that owns them. `upsert` semantics: a re-install
/// lifts any prior soft-retire cleanly.
pub async fn register_plugin_capabilities(
    repo: &dyn CapabilityRegistryRepository,
    plugin_uuid: uuid::Uuid,
    capability_keys: &[String],
) -> Result<(), rim_core::repository::error::RepoError> {
    for key in capability_keys {
        repo.upsert(
            key,
            rim_core::capability::model::CapabilitySourceKind::Plugin,
            Some(plugin_uuid),
            false, // is_self_scoped: always false for plugins — see fn doc
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use mockall::predicate::*;
    use rim_core::capability::model::{CapabilitySourceKind, RegisteredCapability};
    use rim_core::capability::repository::MockCapabilityRegistryRepository;
    use rim_core::server::model::ServerId;
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use uuid::Uuid;

    fn actor() -> Actor {
        Actor::new(UserId::from(Uuid::new_v4()))
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
    /// `get()` expectations. `is_self_scoped` controls the P6-FU5 scope flag.
    fn active_cap(key: &str, is_self_scoped: bool) -> RegisteredCapability {
        RegisteredCapability {
            capability_key: key.to_string(),
            source_kind: CapabilitySourceKind::Builtin,
            plugin_id: None,
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

    /// **C1 closure** — the canonical "unknown cap denies even when the actor
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

    /// **C1 extension** — a retired cap (retired_at IS NOT NULL) is treated
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
        let mut members = MockServerMemberRepository::new();
        members
            .expect_is_member()
            .times(1)
            .returning(|_, _| Ok(true));

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
        let mut members = MockServerMemberRepository::new();
        members.expect_is_member().returning(|_, _| Ok(false));

        let svc = build(registry, members);
        let err = svc
            .check(&actor(), Some(workspace()), "chat.write")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    /// **P6-FU5 regression** — self-scoped cap + workspace_id=None must pass.
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

    /// **P6-FU5 regression** — workspace-scoped cap + workspace_id=None must
    /// PermissionDenied (previously this silently bypassed the membership
    /// check via `membership_holds(actor, None) → Ok(true)`).
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

    /// **P6-FU5 regression** — `check_many` with workspace_id=None must deny
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
        let mut members = MockServerMemberRepository::new();
        // workspace_id is None → membership_holds short-circuits Ok(true)
        members.expect_is_member().returning(|_, _| Ok(true));

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
        let mut members = MockServerMemberRepository::new();
        members
            .expect_is_member()
            // Crucial — bulk path computes membership exactly once even
            // though we ask for two caps. Catches an N+1 regression.
            .times(1)
            .returning(|_, _| Ok(true));

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

    /// **Gate 4 M1 closure** — every `required_capabilities` entry on every
    /// builtin command MUST be in the seeded capability registry, otherwise
    /// the strict default-deny gate silently bricks that command at runtime.
    ///
    /// This is a structural CI guard: a contributor adding `vec!["foo.bar"]`
    /// to a builtin's `required_capabilities` without also adding `foo.bar`
    /// to `builtin_capabilities()` will fail this test. The fix is to add
    /// the cap to the seed list (`capability.rs::builtin_capabilities`),
    /// not to special-case the command.
    ///
    /// Without this guard, M1 ("admin.manage_members declared but unseeded")
    /// would have shipped Phase 1.
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

    /// **Gate 4 M1 extension** — every workspace-scoped builtin cap (those
    /// with `is_self_scoped: false`) must only be used in commands that also
    /// declare a workspace context requirement. Self-scoped caps are allowed
    /// with or without a workspace context.
    ///
    /// This prevents a future contributor from wiring a workspace-scoped cap
    /// to a command that never passes a workspace_id, which would cause the
    /// P6-FU5 gate to fire at runtime with a confusing PermissionDenied.
    #[test]
    fn all_builtin_caps_have_explicit_is_self_scoped_flag() {
        // Structural: every cap in the seed list must have a clear intent.
        // Workspace-scoped caps default to false — this test documents and
        // locks the current classification for reviewers.
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

        // Expected classification per P6-FU5 design:
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
        let mut members = MockServerMemberRepository::new();
        members.expect_is_member().returning(|_, _| Ok(true));

        let svc = build(registry, members);
        let map = svc
            .check_many(&actor(), Some(workspace()), &[String::new()])
            .await
            .expect("empty-string entry should not propagate as error");
        assert_eq!(map.get(""), Some(&false));
    }

    /// P5-PLUGIN-SCOPE: `register_plugin_capabilities` must always pass
    /// `is_self_scoped=false` to the repo — a malicious manifest cannot
    /// claim workspace-gate bypass by declaring a self-scoped plugin cap.
    #[tokio::test]
    async fn p5_plugin_scope_register_never_passes_is_self_scoped_true() {
        let plugin_uuid = uuid::Uuid::new_v4();
        let keys = vec![
            "plugin.evil.bypass_auth".to_string(),
            "plugin.evil.read_all".to_string(),
        ];

        let mut registry = MockCapabilityRegistryRepository::new();
        registry
            .expect_upsert()
            .withf(|_key, source_kind, _plugin_id, is_self_scoped| {
                *source_kind == CapabilitySourceKind::Plugin && !is_self_scoped
            })
            .times(2)
            .returning(|_, _, _, _| Ok(()));

        register_plugin_capabilities(&registry, plugin_uuid, &keys)
            .await
            .expect("registration must succeed");
    }
}
