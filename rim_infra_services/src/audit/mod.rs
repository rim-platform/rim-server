use std::sync::Arc;

use async_trait::async_trait;

use rim_core::audit::repository::{AuditLogEntryRecord, AuditLogListQuery, AuditLogRepository};
use rim_core::server::model::ServerId;
use rim_services::audit::dto::{
    AuditLogEntryDto, AuditLogPage, AuditLogPageInfo, AuditOutcome, ListAuditLogQuery,
};
use rim_services::audit::service::AuditLogService;
use rim_services::capability::service::CapabilityService;
use rim_services::command::verb::TypedVerb;
use rim_services::{Actor, ServiceError, ServiceResult};

pub struct DefaultAuditLogService {
    repo: Arc<dyn AuditLogRepository>,
    capability_service: Arc<dyn CapabilityService>,
}

impl DefaultAuditLogService {
    pub fn new(
        repo: Arc<dyn AuditLogRepository>,
        capability_service: Arc<dyn CapabilityService>,
    ) -> Self {
        Self {
            repo,
            capability_service,
        }
    }
}

#[async_trait]
impl AuditLogService for DefaultAuditLogService {
    async fn write_entry(&self, entry: AuditLogEntryDto) -> ServiceResult<()> {
        let record = AuditLogEntryRecord {
            id: entry.id,
            actor_user_id: entry.actor_user_id,
            server_id: entry.server_id,
            command_id: entry.command_id,
            initiator: entry.initiator,
            args_redacted: entry.args_redacted,
            sensitivity: entry.sensitivity,
            dangerous: entry.dangerous,
            confirmation_kind: entry.confirmation_kind,
            typed_verb: entry.typed_verb.map(|v| v.as_str().to_string()),
            outcome: entry.outcome.as_str().to_string(),
            error_kind: entry.error_kind,
            capability_check_outcome: entry.capability_check_outcome,
            confirmation_outcome: entry.confirmation_outcome,
            parent_audit_id: entry.parent_audit_id,
            created_at: entry.created_at,
        };
        self.repo.insert(record).await.map_err(ServiceError::from)
    }

    /// Gate logic for `list_entries`.
    ///
    /// A cross-user read must be scoped to a workspace where the caller holds
    /// `audit.read`, which only its Owners and Moderators do; supplying another
    /// user's `actor_user_id` never widens access.
    ///
    /// Branching on `(actor_user_id, server_id)`:
    ///
    /// | actor_user_id     | server_id  | Allowed?                                          |
    /// |-------------------|------------|---------------------------------------------------|
    /// | Some(caller)      | _          | Self-read; requires `audit.read.self` (no ws req) |
    /// | Some(other)       | Some(W)    | Cross-user read; requires `audit.read` in W, i.e. |
    /// |                   |            | caller is Owner or Moderator of W (checked by the |
    /// |                   |            | cap svc). The actor_user_id filter is STRIPPED    |
    /// |                   |            | from the repo query — returns full workspace dump.|
    /// |                   |            | Prevents targeted enumeration ("did Alice act     |
    /// |                   |            | in W?") while preserving the authorised read.     |
    /// | Some(other)       | None       | **Forbidden** — cross-user, unscoped              |
    /// | None              | Some(W)    | Workspace read; requires `audit.read` in W        |
    /// |                   |            | (Owner or Moderator of W)                         |
    /// | None              | None       | **Forbidden** — too broad                         |
    async fn list_entries(
        &self,
        caller: Actor,
        query: ListAuditLogQuery,
    ) -> ServiceResult<AuditLogPage> {
        let caller_uuid: uuid::Uuid = caller.user_id.into();
        let workspace_id = query.server_id.map(ServerId::from);

        // Track whether the cross-user+workspace branch fired so we can
        // strip the targeted actor_user_id from the repo query below.
        let mut strip_actor_filter = false;

        // SAFETY: keep in sync with the doc table above. The arm order encodes
        // precedence: the self-read guard fires first (uid == caller_uuid)
        // so a call with (actor_user_id=self, server_id=W) always hits the
        // self arm, never the cross-user arm. Reordering arms or changing
        // the guard condition changes the security boundary — update the
        // doc table in lockstep.
        match (query.actor_user_id, workspace_id) {
            // Self-read: actor_user_id == caller's own id, any server_id OK.
            (Some(uid), _) if uid == caller_uuid => {
                // `audit.read.self` is a self-scoped capability — workspace_id
                // is intentionally None here because self-reads don't require
                // workspace membership (a user can read their own history even
                // after leaving a workspace).
                self.capability_service
                    .check(&caller, None, "audit.read.self")
                    .await?;
            }

            // Cross-user read scoped to a workspace: requires audit.read in W.
            // CapabilityService.check requires the caller to be an Owner or
            // Moderator of W for this cap.
            //
            // Strip the targeted actor_user_id from the repo
            // query to prevent enumeration ("did Alice ever act in W?"). The caller
            // is authorised to see all workspace activity; narrowing server-side by
            // a specific victim's id is a separate enumeration primitive that offers
            // no legitimate use case beyond a full workspace dump + client-side filter.
            (Some(other), Some(ws)) => {
                self.capability_service
                    .check(&caller, Some(ws), "audit.read")
                    .await?;
                tracing::warn!(
                    target: "audit",
                    event = "cross_user_filter_stripped",
                    caller_id = %caller.user_id,
                    requested_user_id = %other,
                    workspace_id = %uuid::Uuid::from(ws),
                    "cross-user actor_user_id filter stripped from audit query; \
                     returning full workspace dump instead of targeted user rows"
                );
                strip_actor_filter = true;
            }

            // Cross-user read without workspace scope: Forbidden.
            (Some(_other), None) => {
                return Err(ServiceError::PermissionDenied);
            }

            // Workspace-scoped read (no specific user filter): requires audit.read in W
            // (Owner or Moderator of W).
            (None, Some(ws)) => {
                self.capability_service
                    .check(&caller, Some(ws), "audit.read")
                    .await?;
            }

            // Completely unscoped read: too broad, Forbidden.
            (None, None) => {
                return Err(ServiceError::PermissionDenied);
            }
        }

        let repo_query = AuditLogListQuery {
            actor_user_id: if strip_actor_filter {
                None
            } else {
                query.actor_user_id
            },
            server_id: query.server_id,
            command_id: query.command_id,
            typed_verb: query.typed_verb,
            since: query.since,
            until: query.until,
            cursor: query.cursor,
            limit: query.limit,
        };

        let page = self.repo.list(repo_query).await?;

        Ok(AuditLogPage {
            items: page
                .items
                .into_iter()
                .map(|r| AuditLogEntryDto {
                    id: r.id,
                    actor_user_id: r.actor_user_id,
                    server_id: r.server_id,
                    command_id: r.command_id,
                    initiator: r.initiator,
                    args_redacted: r.args_redacted,
                    sensitivity: r.sensitivity,
                    dangerous: r.dangerous,
                    confirmation_kind: r.confirmation_kind,
                    typed_verb: r.typed_verb.as_deref().map(TypedVerb::from),
                    outcome: AuditOutcome::try_from(r.outcome.as_str())
                        .unwrap_or(AuditOutcome::Error),
                    error_kind: r.error_kind,
                    capability_check_outcome: r.capability_check_outcome,
                    confirmation_outcome: r.confirmation_outcome,
                    parent_audit_id: r.parent_audit_id,
                    created_at: r.created_at,
                })
                .collect(),
            next_cursor: page.next_cursor,
            page_info: AuditLogPageInfo {
                has_next_page: page.has_next_page,
                has_prev_page: page.has_prev_page,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::audit::repository::{
        AuditLogEntryRecord, AuditLogPageRecord, MockAuditLogRepository,
    };
    use rim_core::user::model::UserId;
    use rim_services::audit::dto::ListAuditLogQuery;
    use rim_services::audit::service::AuditLogService;
    use rim_services::capability::service::MockCapabilityService;
    use rim_services::{Actor, ServiceError};

    use super::DefaultAuditLogService;

    fn alice_id() -> uuid::Uuid {
        uuid::Uuid::from_u128(1)
    }

    fn mallory_id() -> uuid::Uuid {
        uuid::Uuid::from_u128(2)
    }

    fn user3_id() -> uuid::Uuid {
        uuid::Uuid::from_u128(3)
    }

    fn workspace1_id() -> uuid::Uuid {
        uuid::Uuid::from_u128(100)
    }

    fn workspace2_id() -> uuid::Uuid {
        uuid::Uuid::from_u128(200)
    }

    fn fake_row(actor_user_id: uuid::Uuid) -> AuditLogEntryRecord {
        AuditLogEntryRecord {
            id: uuid::Uuid::now_v7(),
            actor_user_id,
            server_id: workspace1_id(),
            command_id: "chat.post_message".to_string(),
            initiator: "palette".to_string(),
            args_redacted: serde_json::Value::Null,
            sensitivity: "safe".to_string(),
            dangerous: false,
            confirmation_kind: "none".to_string(),
            typed_verb: None,
            outcome: "success".to_string(),
            error_kind: None,
            capability_check_outcome: None,
            confirmation_outcome: None,
            parent_audit_id: None,
            created_at: chrono::Utc::now(),
        }
    }

    fn ok_repo() -> MockAuditLogRepository {
        let mut repo = MockAuditLogRepository::new();
        repo.expect_list().returning(|_| {
            Ok(AuditLogPageRecord {
                items: vec![],
                next_cursor: None,
                has_next_page: false,
                has_prev_page: false,
            })
        });
        repo
    }

    // ── Access matrix tests ───────────────────────────────────────────────────

    /// actor_user_id=B, server_id=None → Forbidden.
    /// Cross-user read without any workspace scope must be denied unconditionally,
    /// before any capability check fires.
    #[tokio::test]
    async fn cross_user_no_server_forbidden() {
        // No capability check should fire — we short-circuit to PermissionDenied.
        let repo = MockAuditLogRepository::new();
        let cap_svc = MockCapabilityService::new();

        let svc = DefaultAuditLogService::new(Arc::new(repo), Arc::new(cap_svc));
        let caller = Actor::new(UserId::from(mallory_id()));

        let result = svc
            .list_entries(
                caller,
                ListAuditLogQuery {
                    actor_user_id: Some(alice_id()),
                    server_id: None,
                    ..Default::default()
                },
            )
            .await;

        assert!(
            matches!(result, Err(ServiceError::PermissionDenied)),
            "cross-user read without workspace scope must be Forbidden: {result:?}"
        );
    }

    /// actor_user_id=B, server_id=W2 where caller is not a member → Forbidden.
    /// The capability check fires for audit.read in W2, which denies non-members.
    #[tokio::test]
    async fn cross_user_non_member_workspace_forbidden() {
        let repo = MockAuditLogRepository::new();
        let mut cap_svc = MockCapabilityService::new();
        // Capability check fires for audit.read with workspace_id=W2, but
        // since the caller is not a member, CapabilityService returns PermissionDenied.
        cap_svc
            .expect_check()
            .withf(|actor, ws_id, cap| {
                actor.user_id == UserId::from(mallory_id())
                    && ws_id
                        .map(|w| uuid::Uuid::from(w) == workspace2_id())
                        .unwrap_or(false)
                    && cap == "audit.read"
            })
            .returning(|_, _, _| Err(ServiceError::PermissionDenied));

        let svc = DefaultAuditLogService::new(Arc::new(repo), Arc::new(cap_svc));
        let caller = Actor::new(UserId::from(mallory_id()));

        let result = svc
            .list_entries(
                caller,
                ListAuditLogQuery {
                    actor_user_id: Some(alice_id()),
                    server_id: Some(workspace2_id()),
                    ..Default::default()
                },
            )
            .await;

        assert!(
            matches!(result, Err(ServiceError::PermissionDenied)),
            "cross-user read in non-member workspace must be Forbidden: {result:?}"
        );
    }

    /// actor_user_id=A (self), server_id=None → Ok with audit.read.self.
    #[tokio::test]
    async fn self_read_no_server_ok() {
        let mut cap_svc = MockCapabilityService::new();
        cap_svc
            .expect_check()
            .withf(|actor, ws_id, cap| {
                actor.user_id == UserId::from(alice_id())
                    && ws_id.is_none()
                    && cap == "audit.read.self"
            })
            .returning(|_, _, _| Ok(()));

        let svc = DefaultAuditLogService::new(Arc::new(ok_repo()), Arc::new(cap_svc));
        let caller = Actor::new(UserId::from(alice_id()));

        let result = svc
            .list_entries(
                caller,
                ListAuditLogQuery {
                    actor_user_id: Some(alice_id()),
                    server_id: None,
                    ..Default::default()
                },
            )
            .await;

        assert!(
            result.is_ok(),
            "self-read must succeed with audit.read.self: {result:?}"
        );
    }

    /// actor_user_id=None, server_id=W1 (caller holds audit.read in W1) → Ok.
    #[tokio::test]
    async fn workspace_read_with_audit_read_ok() {
        let mut cap_svc = MockCapabilityService::new();
        cap_svc
            .expect_check()
            .withf(|actor, ws_id, cap| {
                actor.user_id == UserId::from(alice_id())
                    && ws_id
                        .map(|w| uuid::Uuid::from(w) == workspace1_id())
                        .unwrap_or(false)
                    && cap == "audit.read"
            })
            .returning(|_, _, _| Ok(()));

        let svc = DefaultAuditLogService::new(Arc::new(ok_repo()), Arc::new(cap_svc));
        let caller = Actor::new(UserId::from(alice_id()));

        let result = svc
            .list_entries(
                caller,
                ListAuditLogQuery {
                    actor_user_id: None,
                    server_id: Some(workspace1_id()),
                    ..Default::default()
                },
            )
            .await;

        assert!(
            result.is_ok(),
            "workspace read with audit.read must succeed: {result:?}"
        );
    }

    /// actor_user_id=None, server_id=None → Forbidden (too broad).
    #[tokio::test]
    async fn unscoped_read_forbidden() {
        let repo = MockAuditLogRepository::new();
        let cap_svc = MockCapabilityService::new();

        let svc = DefaultAuditLogService::new(Arc::new(repo), Arc::new(cap_svc));
        let caller = Actor::new(UserId::from(alice_id()));

        let result = svc
            .list_entries(
                caller,
                ListAuditLogQuery {
                    actor_user_id: None,
                    server_id: None,
                    ..Default::default()
                },
            )
            .await;

        assert!(
            matches!(result, Err(ServiceError::PermissionDenied)),
            "unscoped read must be Forbidden: {result:?}"
        );
    }

    // ── Enumeration regression tests ──────────────────────────────────────────

    /// A cross-user+workspace read strips the actor_user_id from the
    /// repo query. The `.withf` on the repo expectation proves the DB never
    /// receives the targeted filter — Mallory cannot enumerate Alice's activity
    /// even when she has `audit.read` in W1.
    #[tokio::test]
    async fn cross_user_workspace_strips_actor_filter() {
        let mut repo = MockAuditLogRepository::new();
        // Repo must be called WITHOUT actor_user_id (None), proving the
        // targeted filter was stripped before the query reached the DB.
        repo.expect_list()
            .withf(|q| q.actor_user_id.is_none())
            .returning(|_| {
                Ok(AuditLogPageRecord {
                    items: vec![],
                    next_cursor: None,
                    has_next_page: false,
                    has_prev_page: false,
                })
            });

        let mut cap_svc = MockCapabilityService::new();
        cap_svc
            .expect_check()
            .withf(|actor, ws_id, cap| {
                actor.user_id == UserId::from(mallory_id())
                    && ws_id
                        .map(|w| uuid::Uuid::from(w) == workspace1_id())
                        .unwrap_or(false)
                    && cap == "audit.read"
            })
            .returning(|_, _, _| Ok(()));

        let svc = DefaultAuditLogService::new(Arc::new(repo), Arc::new(cap_svc));
        let caller = Actor::new(UserId::from(mallory_id()));

        let result = svc
            .list_entries(
                caller,
                ListAuditLogQuery {
                    actor_user_id: Some(alice_id()),
                    server_id: Some(workspace1_id()),
                    ..Default::default()
                },
            )
            .await;

        assert!(
            result.is_ok(),
            "cross-user+workspace read must succeed (audit.read granted) but strip filter: {result:?}"
        );
    }

    /// A self-read still passes actor_user_id to the repo (no stripping).
    #[tokio::test]
    async fn self_read_preserves_actor_filter() {
        let mut repo = MockAuditLogRepository::new();
        // Self-read: repo receives actor_user_id = Alice's id.
        repo.expect_list()
            .withf(move |q| q.actor_user_id == Some(alice_id()))
            .returning(|_| {
                Ok(AuditLogPageRecord {
                    items: vec![],
                    next_cursor: None,
                    has_next_page: false,
                    has_prev_page: false,
                })
            });

        let mut cap_svc = MockCapabilityService::new();
        cap_svc
            .expect_check()
            .withf(|actor, ws_id, cap| {
                actor.user_id == UserId::from(alice_id())
                    && ws_id.is_none()
                    && cap == "audit.read.self"
            })
            .returning(|_, _, _| Ok(()));

        let svc = DefaultAuditLogService::new(Arc::new(repo), Arc::new(cap_svc));
        let caller = Actor::new(UserId::from(alice_id()));

        let result = svc
            .list_entries(
                caller,
                ListAuditLogQuery {
                    actor_user_id: Some(alice_id()),
                    server_id: None,
                    ..Default::default()
                },
            )
            .await;

        assert!(
            result.is_ok(),
            "self-read must succeed and preserve actor_user_id filter: {result:?}"
        );
    }

    /// A cross-user+workspace read returns the full workspace dump — rows from
    /// all users in the workspace, NOT only the requested target user.
    /// Mallory queries `target_user=Alice, server_id=W1`; the repo (simulating
    /// rows from Alice, Mallory, and User3) returns all three. Service must
    /// pass them through unchanged — no post-filter on actor_user_id.
    #[tokio::test]
    async fn cross_user_workspace_returns_full_workspace_dump() {
        let mut repo = MockAuditLogRepository::new();
        // Repo returns rows from three different users — simulates a real workspace.
        repo.expect_list()
            .withf(|q| q.actor_user_id.is_none())
            .returning(|_| {
                Ok(AuditLogPageRecord {
                    items: vec![
                        fake_row(alice_id()),
                        fake_row(mallory_id()),
                        fake_row(user3_id()),
                    ],
                    next_cursor: None,
                    has_next_page: false,
                    has_prev_page: false,
                })
            });

        let mut cap_svc = MockCapabilityService::new();
        cap_svc.expect_check().returning(|_, _, _| Ok(()));

        let svc = DefaultAuditLogService::new(Arc::new(repo), Arc::new(cap_svc));
        let caller = Actor::new(UserId::from(mallory_id()));

        let result = svc
            .list_entries(
                caller,
                ListAuditLogQuery {
                    actor_user_id: Some(alice_id()),
                    server_id: Some(workspace1_id()),
                    ..Default::default()
                },
            )
            .await
            .expect("cross-user+workspace read must succeed");

        let returned_users: std::collections::HashSet<uuid::Uuid> =
            result.items.iter().map(|r| r.actor_user_id).collect();
        assert!(
            returned_users.contains(&alice_id()),
            "full dump must include Alice's rows"
        );
        assert!(
            returned_users.contains(&mallory_id()),
            "full dump must include Mallory's rows"
        );
        assert!(
            returned_users.contains(&user3_id()),
            "full dump must include User3's rows — not just the targeted user"
        );
    }

    /// A cross-user+workspace read where the victim has NO rows in W must
    /// return the full workspace dump (non-empty), NOT an empty result that
    /// would leak victim absence information.
    #[tokio::test]
    async fn cross_user_workspace_no_victim_rows_returns_workspace_dump_not_empty() {
        let mut repo = MockAuditLogRepository::new();
        // Victim (alice) has NO rows in W; workspace has rows from other users.
        // The repo is queried without actor_user_id filter, so it returns
        // the workspace rows from the other users.
        repo.expect_list()
            .withf(|q| q.actor_user_id.is_none())
            .returning(|_| {
                Ok(AuditLogPageRecord {
                    items: vec![fake_row(mallory_id()), fake_row(user3_id())],
                    next_cursor: None,
                    has_next_page: false,
                    has_prev_page: false,
                })
            });

        let mut cap_svc = MockCapabilityService::new();
        cap_svc.expect_check().returning(|_, _, _| Ok(()));

        let svc = DefaultAuditLogService::new(Arc::new(repo), Arc::new(cap_svc));
        let caller = Actor::new(UserId::from(mallory_id()));

        let result = svc
            .list_entries(
                caller,
                ListAuditLogQuery {
                    // Mallory is probing for Alice's activity in W1.
                    actor_user_id: Some(alice_id()),
                    server_id: Some(workspace1_id()),
                    ..Default::default()
                },
            )
            .await
            .expect("read must succeed");

        assert!(
            !result.items.is_empty(),
            "response must be the full workspace dump, not an empty result \
             that would leak victim absence"
        );
        let returned_users: std::collections::HashSet<uuid::Uuid> =
            result.items.iter().map(|r| r.actor_user_id).collect();
        assert!(
            !returned_users.contains(&alice_id()),
            "Alice genuinely has no rows — but response is non-empty workspace dump"
        );
    }

    // ── Legacy regression tests (updated for new gate) ────────────────────────

    /// Regression: mallory (no audit.read in W1) passes alice's user_id +
    /// workspace W1. Must be denied via the capability check.
    #[tokio::test]
    async fn privilege_escalation_blocked() {
        let repo = MockAuditLogRepository::new();

        let mut cap_svc = MockCapabilityService::new();
        // Cross-user+server path: fires audit.read check with workspace_id=W1.
        // Mallory is not a member of W1 so the cap svc denies.
        cap_svc
            .expect_check()
            .withf(|actor, _, cap| {
                actor.user_id == UserId::from(mallory_id()) && cap == "audit.read"
            })
            .returning(|_, _, _| Err(ServiceError::PermissionDenied));

        let svc = DefaultAuditLogService::new(Arc::new(repo), Arc::new(cap_svc));
        let mallory_caller = Actor::new(UserId::from(mallory_id()));

        // Mallory tries to read alice's audit log scoped to W1.
        let result = svc
            .list_entries(
                mallory_caller,
                ListAuditLogQuery {
                    actor_user_id: Some(alice_id()),
                    server_id: Some(workspace1_id()),
                    ..Default::default()
                },
            )
            .await;

        assert!(
            matches!(result, Err(ServiceError::PermissionDenied)),
            "mallory must be denied even when spoofing alice's actor_user_id: {result:?}"
        );
    }

    /// Happy path — cross-user read by a caller holding `audit.read` in W1 must succeed.
    ///
    /// Mallory has `audit.read` in W1. She queries `actor_user_id=Alice,
    /// server_id=W1`. The cap check passes and the service returns Ok.
    /// This test pins the happy path so a future arm-swap refactor (e.g.
    /// accidentally flipping the `Some(other)` / `None` arms) breaks the build
    /// rather than silently allowing or silently denying.
    #[tokio::test]
    async fn cross_user_member_workspace_ok() {
        let mut repo = MockAuditLogRepository::new();
        repo.expect_list().returning(|_| {
            Ok(AuditLogPageRecord {
                items: vec![],
                next_cursor: None,
                has_next_page: false,
                has_prev_page: false,
            })
        });

        let mut cap_svc = MockCapabilityService::new();
        // Cross-user+workspace: expect audit.read with workspace_id=W1, exactly once.
        cap_svc
            .expect_check()
            .times(1)
            .withf(|actor, ws_id, cap| {
                actor.user_id == UserId::from(mallory_id())
                    && ws_id
                        .map(|w| uuid::Uuid::from(w) == workspace1_id())
                        .unwrap_or(false)
                    && cap == "audit.read"
            })
            .returning(|_, _, _| Ok(()));

        let svc = DefaultAuditLogService::new(Arc::new(repo), Arc::new(cap_svc));
        let result = svc
            .list_entries(
                Actor::new(UserId::from(mallory_id())),
                ListAuditLogQuery {
                    actor_user_id: Some(alice_id()),
                    server_id: Some(workspace1_id()),
                    ..Default::default()
                },
            )
            .await;

        assert!(
            result.is_ok(),
            "cross-user read by workspace member must succeed: {result:?}"
        );
        assert!(
            result.unwrap().items.is_empty(),
            "mock repo returned no rows — items must be empty"
        );
    }

    /// Precedence — `(actor_user_id=self, server_id=Some(W))` must
    /// hit the self-read arm, NOT the cross-user arm.
    ///
    /// The self arm fires first because of the `uid == caller_uuid` guard.
    /// This test proves the precedence is stable: if the guard is ever
    /// removed or reordered the mockall expectation fires on `audit.read`
    /// with `workspace_id=Some(W1)` instead of `audit.read.self` with
    /// `workspace_id=None`, failing the test immediately.
    #[tokio::test]
    async fn self_arm_takes_precedence() {
        let mut cap_svc = MockCapabilityService::new();
        // Wrong arm: audit.read with workspace_id=Some(W1) must NEVER fire.
        // If the self-arm guard is removed or reordered this expectation fires,
        // failing the test immediately.
        cap_svc
            .expect_check()
            .never()
            .withf(|_actor, ws_id, cap| ws_id.is_some() && cap == "audit.read");
        // Self arm: audit.read.self with workspace_id=None — exactly once.
        cap_svc
            .expect_check()
            .times(1)
            .withf(|actor, ws_id, cap| {
                actor.user_id == UserId::from(alice_id())
                    && ws_id.is_none()
                    && cap == "audit.read.self"
            })
            .returning(|_, _, _| Ok(()));

        let svc = DefaultAuditLogService::new(Arc::new(ok_repo()), Arc::new(cap_svc));
        let result = svc
            .list_entries(
                Actor::new(UserId::from(alice_id())),
                ListAuditLogQuery {
                    actor_user_id: Some(alice_id()),
                    server_id: Some(workspace1_id()), // server_id present — self arm must still win
                    ..Default::default()
                },
            )
            .await;

        assert!(
            result.is_ok(),
            "self-read with server_id must hit self arm, not cross-user arm: {result:?}"
        );
    }

    /// Happy path: caller reads their own entries (self-read path → audit.read.self).
    #[tokio::test]
    async fn authorized_caller_can_list() {
        let mut cap_svc = MockCapabilityService::new();
        // Self-read: fires audit.read.self with workspace_id=None.
        cap_svc
            .expect_check()
            .withf(|_, ws_id, cap| ws_id.is_none() && cap == "audit.read.self")
            .returning(|_, _, _| Ok(()));

        let svc = DefaultAuditLogService::new(Arc::new(ok_repo()), Arc::new(cap_svc));
        let alice_caller = Actor::new(UserId::from(alice_id()));

        let result = svc
            .list_entries(
                alice_caller,
                ListAuditLogQuery {
                    actor_user_id: Some(alice_id()),
                    ..Default::default()
                },
            )
            .await;

        assert!(result.is_ok(), "authorized caller must succeed: {result:?}");
        assert!(result.unwrap().items.is_empty());
    }
    // ── Role matrix through the real capability gate ──────────────────────────

    /// Audit service over the real `DefaultCapabilityService`: the registry knows
    /// `audit.read` and `audit.read.self`, and Mallory holds `role` in W1 (`None`:
    /// not a member). The repo answers any `list` with an empty page.
    fn svc_with_role(
        role: Option<rim_core::server_member::model::ServerMemberRole>,
    ) -> DefaultAuditLogService {
        use rim_core::capability::model::RegisteredCapability;
        use rim_core::capability::repository::MockCapabilityRegistryRepository;
        use rim_core::server_member::model::{ServerMember, ServerMemberId};
        use rim_core::server_member::repository::MockServerMemberRepository;

        use crate::capability::{CapabilityRepos, DefaultCapabilityService};

        let mut registry = MockCapabilityRegistryRepository::new();
        registry.expect_get().returning(|key| {
            Ok(
                matches!(key, "audit.read" | "audit.read.self").then(|| RegisteredCapability {
                    capability_key: key.to_string(),
                    registered_at: chrono::Utc::now(),
                    retired_at: None,
                    is_self_scoped: key == "audit.read.self",
                }),
            )
        });
        let mut members = MockServerMemberRepository::new();
        members
            .expect_get_member()
            .returning(move |server_id, user_id| {
                Ok(role
                    .filter(|_| uuid::Uuid::from(server_id) == workspace1_id())
                    .map(|member_role| ServerMember {
                        id: ServerMemberId::new(),
                        server_id,
                        user_id,
                        member_role,
                        joined_at: chrono::Utc::now(),
                    }))
            });
        let cap_svc = DefaultCapabilityService::new(CapabilityRepos {
            capability_registry_repo: Arc::new(registry),
            server_member_repo: Arc::new(members),
        });
        DefaultAuditLogService::new(Arc::new(ok_repo()), Arc::new(cap_svc))
    }

    /// Mallory's `list_entries` result for `(actor_user_id, server_id)` while holding `role` in W1.
    async fn list_as_mallory(
        role: Option<rim_core::server_member::model::ServerMemberRole>,
        actor_user_id: Option<uuid::Uuid>,
        server_id: Option<uuid::Uuid>,
    ) -> Result<(), ServiceError> {
        svc_with_role(role)
            .list_entries(
                Actor::new(UserId::from(mallory_id())),
                ListAuditLogQuery {
                    actor_user_id,
                    server_id,
                    ..Default::default()
                },
            )
            .await
            .map(|_| ())
    }

    /// The workspace arm `(None, Some(W1))` and the cross-user arm `(Some(other), Some(W1))`
    /// succeed for Owners and Moderators of W1 and are denied for Members, Guests and
    /// non-members.
    #[tokio::test]
    async fn workspace_and_cross_user_arms_require_owner_or_moderator() {
        use rim_core::server_member::model::ServerMemberRole;

        for (role, allowed) in [
            (Some(ServerMemberRole::Owner), true),
            (Some(ServerMemberRole::Moderator), true),
            (Some(ServerMemberRole::Member), false),
            (Some(ServerMemberRole::Guest), false),
            (None, false),
        ] {
            for actor_user_id in [None, Some(alice_id())] {
                let result = list_as_mallory(role, actor_user_id, Some(workspace1_id())).await;
                if allowed {
                    assert!(result.is_ok(), "{role:?} {actor_user_id:?}: {result:?}");
                } else {
                    assert!(
                        matches!(result, Err(ServiceError::PermissionDenied)),
                        "{role:?} {actor_user_id:?}: {result:?}"
                    );
                }
            }
        }
    }

    /// The self-read arm passes for every role, inside W1 and without a workspace, and for a
    /// caller who is no member at all.
    #[tokio::test]
    async fn self_read_arm_unaffected_by_role() {
        use rim_core::server_member::model::ServerMemberRole;

        for role in [
            Some(ServerMemberRole::Owner),
            Some(ServerMemberRole::Moderator),
            Some(ServerMemberRole::Member),
            Some(ServerMemberRole::Guest),
            None,
        ] {
            for server_id in [None, Some(workspace1_id())] {
                let result = list_as_mallory(role, Some(mallory_id()), server_id).await;
                assert!(result.is_ok(), "{role:?} {server_id:?}: {result:?}");
            }
        }
    }

    /// The unscoped arms stay forbidden even for an Owner.
    #[tokio::test]
    async fn unscoped_arms_forbidden_for_owner() {
        use rim_core::server_member::model::ServerMemberRole;

        let owner = Some(ServerMemberRole::Owner);
        for actor_user_id in [None, Some(alice_id())] {
            let result = list_as_mallory(owner, actor_user_id, None).await;
            assert!(
                matches!(result, Err(ServiceError::PermissionDenied)),
                "{actor_user_id:?}: {result:?}"
            );
        }
    }
}
