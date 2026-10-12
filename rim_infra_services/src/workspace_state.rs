use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use regex::Regex;

use rim_core::server::model::ServerId;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_core::workspace_state::model::{NewWorkspaceState, WorkspaceStateScope};
use rim_core::workspace_state::repository::WorkspaceStateRepository;
use rim_services::events::{Broadcaster, WorkspaceEvent};
use rim_services::workspace_state::dto::{WorkspaceStateDto, WorkspaceStateScopeDto};
use rim_services::workspace_state::request::{
    DeleteWorkspaceStateCmd, GetWorkspaceStateQuery, IncrementWorkspaceStateCmd,
    ListWorkspaceStateQuery, SetWorkspaceStateCmd,
};
use rim_services::workspace_state::service::WorkspaceStateService;
use rim_services::{ServiceError, ServiceResult};

const MAX_SERVER_ENTRIES: u64 = 10_000;
const MAX_USER_ENTRIES: u64 = 1_000;
const MAX_VALUE_SIZE: usize = 64 * 1024; // 64 KB
const DEFAULT_LIST_LIMIT: u64 = 100;
const MAX_LIST_LIMIT: u64 = 500;
const MAX_TTL_SECONDS: i64 = 30 * 24 * 3600; // 30 days
const MAX_PREFIX_LENGTH: usize = 512;
const MAX_INCREMENT_DELTA: i64 = 1_000_000;
const MIN_INCREMENT_DELTA: i64 = -1_000_000;

// Any workspace member may read and write every key of the workspace scope and of their own
// user scope. Writes are bounded by the entry counts, value size, TTL and increment limits above.

#[derive(Clone)]
pub struct WorkspaceStateRepos {
    pub workspace_state_repo: Arc<dyn WorkspaceStateRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
}

#[derive(Clone)]
pub struct DefaultWorkspaceStateService {
    repos: WorkspaceStateRepos,
    broadcaster: Arc<dyn Broadcaster>,
    /// The relay's direct space, which has no workspace state.
    direct_space_id: Option<ServerId>,
}

impl DefaultWorkspaceStateService {
    pub fn new(repos: WorkspaceStateRepos, broadcaster: Arc<dyn Broadcaster>) -> Self {
        Self {
            repos,
            broadcaster,
            direct_space_id: None,
        }
    }

    /// The relay's direct space, or `None` where none is wired.
    pub fn with_direct_space(mut self, direct_space_id: Option<ServerId>) -> Self {
        self.direct_space_id = direct_space_id;
        self
    }

    fn validate_key(key: &str) -> ServiceResult<()> {
        static KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^[a-z][a-z0-9_.:-]{0,510}[a-z0-9]$").expect("valid regex")
        });

        if !KEY_REGEX.is_match(key) {
            return Err(ServiceError::Validation(format!(
                "invalid key format: must match ^[a-z][a-z0-9_.:-]{{0,510}}[a-z0-9]$, got `{}`",
                &key[..key.len().min(64)]
            )));
        }

        // SECURITY: Reserve system.* namespace for server-side services only.
        if key.starts_with("system.") {
            return Err(ServiceError::Validation(
                "keys starting with 'system.' are reserved".into(),
            ));
        }

        Ok(())
    }

    fn validate_value_size(value: &serde_json::Value) -> ServiceResult<()> {
        let size = serde_json::to_vec(value).map(|v| v.len()).unwrap_or(0);
        if size > MAX_VALUE_SIZE {
            return Err(ServiceError::Validation(format!(
                "value exceeds maximum size of {MAX_VALUE_SIZE} bytes ({size} bytes)"
            )));
        }
        Ok(())
    }

    fn resolve_scope(
        scope_dto: WorkspaceStateScopeDto,
        user_id: UserId,
    ) -> (WorkspaceStateScope, Option<UserId>) {
        match scope_dto {
            WorkspaceStateScopeDto::Server => (WorkspaceStateScope::Server, None),
            WorkspaceStateScopeDto::User => (WorkspaceStateScope::User, Some(user_id)),
        }
    }

    /// `Validation` in the direct space, else `PermissionDenied` for a non-member.
    async fn ensure_member(&self, server_id: ServerId, user_id: UserId) -> ServiceResult<()> {
        crate::util::ensure_workspace_features(self.direct_space_id, server_id)?;
        crate::util::ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            server_id,
            user_id,
        )
        .await
    }

    fn broadcast_changed(&self, server_id: ServerId, dto: &WorkspaceStateDto) {
        self.broadcaster
            .broadcast(WorkspaceEvent::WorkspaceStateChanged {
                workspace_id: server_id,
                key: dto.key.clone(),
                value: dto.value.clone(),
                scope: dto.scope,
                user_id: dto.user_id,
                version: dto.version,
            });
    }

    fn broadcast_deleted(
        &self,
        server_id: ServerId,
        key: &str,
        scope: WorkspaceStateScopeDto,
        user_id: Option<UserId>,
    ) {
        self.broadcaster
            .broadcast(WorkspaceEvent::WorkspaceStateDeleted {
                workspace_id: server_id,
                key: key.to_string(),
                scope,
                user_id,
            });
    }
}

#[async_trait]
impl WorkspaceStateService for DefaultWorkspaceStateService {
    async fn get(&self, query: GetWorkspaceStateQuery) -> ServiceResult<Option<WorkspaceStateDto>> {
        self.ensure_member(query.server_id, query.actor.user_id)
            .await?;
        let (scope, user_id) = Self::resolve_scope(query.scope, query.actor.user_id);
        let entry = self
            .repos
            .workspace_state_repo
            .get_by_key(query.server_id, scope, user_id, &query.key)
            .await?;
        Ok(entry.map(Into::into))
    }

    async fn list(&self, query: ListWorkspaceStateQuery) -> ServiceResult<Vec<WorkspaceStateDto>> {
        self.ensure_member(query.server_id, query.actor.user_id)
            .await?;
        if query.prefix.len() > MAX_PREFIX_LENGTH {
            return Err(ServiceError::Validation(format!(
                "prefix exceeds maximum length of {MAX_PREFIX_LENGTH} characters"
            )));
        }
        let (scope, user_id) = Self::resolve_scope(query.scope, query.actor.user_id);
        let limit = query
            .limit
            .unwrap_or(DEFAULT_LIST_LIMIT)
            .min(MAX_LIST_LIMIT);
        let entries = self
            .repos
            .workspace_state_repo
            .list_by_prefix(query.server_id, scope, user_id, &query.prefix, limit)
            .await?;
        Ok(entries.into_iter().map(Into::into).collect())
    }

    async fn set(&self, cmd: SetWorkspaceStateCmd) -> ServiceResult<WorkspaceStateDto> {
        self.ensure_member(cmd.server_id, cmd.actor.user_id).await?;
        Self::validate_key(&cmd.key)?;
        Self::validate_value_size(&cmd.value)?;

        if let Some(ttl) = cmd.ttl_seconds {
            if ttl <= 0 {
                return Err(ServiceError::Validation(
                    "ttl_seconds must be positive".into(),
                ));
            }
            if ttl > MAX_TTL_SECONDS {
                return Err(ServiceError::Validation(format!(
                    "ttl_seconds exceeds maximum of {MAX_TTL_SECONDS} seconds (30 days)"
                )));
            }
        }

        let (scope, user_id) = Self::resolve_scope(cmd.scope, cmd.actor.user_id);

        // Check quota
        let count = self
            .repos
            .workspace_state_repo
            .count(cmd.server_id, scope, user_id)
            .await?;

        let max = match scope {
            WorkspaceStateScope::Server => MAX_SERVER_ENTRIES,
            WorkspaceStateScope::User => MAX_USER_ENTRIES,
        };

        // Only enforce quota if this is a new entry (check if it exists first)
        let existing = self
            .repos
            .workspace_state_repo
            .get_by_key(cmd.server_id, scope, user_id, &cmd.key)
            .await?;

        if existing.is_none() && count >= max {
            return Err(ServiceError::Validation(format!(
                "quota exceeded: maximum {max} entries for {scope} scope",
                scope = scope.as_str()
            )));
        }

        let expires_at = cmd
            .ttl_seconds
            .map(|ttl| chrono::Utc::now() + chrono::Duration::seconds(ttl));

        let entry = self
            .repos
            .workspace_state_repo
            .upsert(NewWorkspaceState {
                server_id: cmd.server_id,
                key: cmd.key,
                value: cmd.value,
                scope,
                user_id,
                created_by: cmd.actor.user_id,
                expires_at,
            })
            .await?;

        let dto: WorkspaceStateDto = entry.into();
        self.broadcast_changed(cmd.server_id, &dto);
        Ok(dto)
    }

    async fn delete(&self, cmd: DeleteWorkspaceStateCmd) -> ServiceResult<bool> {
        self.ensure_member(cmd.server_id, cmd.actor.user_id).await?;
        let (scope, user_id) = Self::resolve_scope(cmd.scope, cmd.actor.user_id);
        let deleted = self
            .repos
            .workspace_state_repo
            .delete_by_key(cmd.server_id, scope, user_id, &cmd.key)
            .await?;

        if deleted {
            self.broadcast_deleted(cmd.server_id, &cmd.key, cmd.scope, user_id);
        }
        Ok(deleted)
    }

    async fn increment(&self, cmd: IncrementWorkspaceStateCmd) -> ServiceResult<WorkspaceStateDto> {
        self.ensure_member(cmd.server_id, cmd.actor.user_id).await?;
        Self::validate_key(&cmd.key)?;

        if cmd.delta < MIN_INCREMENT_DELTA || cmd.delta > MAX_INCREMENT_DELTA {
            return Err(ServiceError::Validation(format!(
                "delta must be between {MIN_INCREMENT_DELTA} and {MAX_INCREMENT_DELTA}"
            )));
        }

        let (scope, user_id) = Self::resolve_scope(cmd.scope, cmd.actor.user_id);

        // Check if key exists — if it does, verify value is numeric
        let existing = self
            .repos
            .workspace_state_repo
            .get_by_key(cmd.server_id, scope, user_id, &cmd.key)
            .await?;

        if let Some(ref entry) = existing {
            if !entry.value.is_number() {
                return Err(ServiceError::Validation(
                    "cannot increment a non-numeric value".into(),
                ));
            }
        } else {
            // New entry — check quota
            let count = self
                .repos
                .workspace_state_repo
                .count(cmd.server_id, scope, user_id)
                .await?;

            let max = match scope {
                WorkspaceStateScope::Server => MAX_SERVER_ENTRIES,
                WorkspaceStateScope::User => MAX_USER_ENTRIES,
            };

            if count >= max {
                return Err(ServiceError::Validation(format!(
                    "quota exceeded: maximum {max} entries for {} scope",
                    scope.as_str()
                )));
            }
        }

        // Atomic increment via single SQL statement — no TOCTOU race.
        let entry = self
            .repos
            .workspace_state_repo
            .increment_by_key(
                cmd.server_id,
                scope,
                user_id,
                &cmd.key,
                cmd.delta,
                cmd.actor.user_id,
            )
            .await?;

        let dto: WorkspaceStateDto = entry.into();
        self.broadcast_changed(cmd.server_id, &dto);
        Ok(dto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::repository::error::RepoResult;
    use rim_core::server_member::model::{ServerMember, ServerMemberRole};
    use rim_core::server_member::repository::ServerMemberRepository;
    use rim_core::workspace_state::model::{WorkspaceState, WorkspaceStateId};
    use rim_services::Actor;

    mockall::mock! {
        WorkspaceStateRepository {}
        #[async_trait]
        impl WorkspaceStateRepository for WorkspaceStateRepository {
            async fn get_by_key(
                &self,
                server_id: ServerId,
                scope: WorkspaceStateScope,
                user_id: Option<UserId>,
                key: &str,
            ) -> RepoResult<Option<WorkspaceState>>;
            async fn list_by_prefix(
                &self,
                server_id: ServerId,
                scope: WorkspaceStateScope,
                user_id: Option<UserId>,
                prefix: &str,
                limit: u64,
            ) -> RepoResult<Vec<WorkspaceState>>;
            async fn count(
                &self,
                server_id: ServerId,
                scope: WorkspaceStateScope,
                user_id: Option<UserId>,
            ) -> RepoResult<u64>;
            async fn upsert(&self, new: NewWorkspaceState) -> RepoResult<WorkspaceState>;
            async fn delete_by_key(
                &self,
                server_id: ServerId,
                scope: WorkspaceStateScope,
                user_id: Option<UserId>,
                key: &str,
            ) -> RepoResult<bool>;
            async fn delete_expired(&self) -> RepoResult<u64>;
            async fn increment_by_key(
                &self,
                server_id: ServerId,
                scope: WorkspaceStateScope,
                user_id: Option<UserId>,
                key: &str,
                delta: i64,
                actor_id: UserId,
            ) -> RepoResult<WorkspaceState>;
        }
    }

    mockall::mock! {
        ServerMemberRepository {}
        #[async_trait]
        impl ServerMemberRepository for ServerMemberRepository {
            async fn get_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<Option<ServerMember>>;
            async fn add_member(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<ServerMember>;
            async fn change_member_role(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<()>;
            async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()>;
            async fn get_members_for_server(&self, server_id: ServerId) -> RepoResult<Vec<ServerMember>>;
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn join_direct_space(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn get_member_standing(&self, server_id: rim_core::server::model::ServerId, user_id: rim_core::user::model::UserId) -> RepoResult<Option<rim_core::server_member::model::MemberStanding>>;
            async fn get_owner_id(&self, server_id: rim_core::server::model::ServerId) -> RepoResult<Option<rim_core::user::model::UserId>>;
        }
    }

    fn make_member_repo() -> MockServerMemberRepository {
        let mut repo = MockServerMemberRepository::new();
        repo.expect_is_member().returning(|_, _| Ok(true));
        repo
    }

    fn make_service(repo: MockWorkspaceStateRepository) -> DefaultWorkspaceStateService {
        DefaultWorkspaceStateService::new(
            WorkspaceStateRepos {
                workspace_state_repo: Arc::new(repo),
                server_member_repo: Arc::new(make_member_repo()),
            },
            Arc::new(rim_services::events::NoopBroadcaster),
        )
    }

    fn make_ws(
        server_id: ServerId,
        key: &str,
        value: serde_json::Value,
        scope: WorkspaceStateScope,
        user_id: Option<UserId>,
    ) -> WorkspaceState {
        WorkspaceState {
            id: WorkspaceStateId::new(),
            server_id,
            key: key.to_string(),
            value,
            scope,
            user_id,
            created_by: UserId::new(),
            updated_by: UserId::new(),
            version: 1,
            expires_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn set_and_get() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let mut repo = MockWorkspaceStateRepository::new();
        let sid = server_id;
        repo.expect_count().returning(|_, _, _| Ok(0));
        repo.expect_get_by_key().returning(|_, _, _, _| Ok(None));
        repo.expect_upsert()
            .returning(move |new| Ok(make_ws(sid, &new.key, new.value, new.scope, new.user_id)));

        let svc = make_service(repo);

        let result = svc
            .set(SetWorkspaceStateCmd {
                actor,
                server_id,
                key: "my.key".to_string(),
                value: serde_json::json!("hello"),
                scope: WorkspaceStateScopeDto::Server,
                ttl_seconds: None,
            })
            .await
            .unwrap();

        assert_eq!(result.key, "my.key");
        assert_eq!(result.value, serde_json::json!("hello"));
    }

    #[tokio::test]
    async fn set_with_ttl() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let mut repo = MockWorkspaceStateRepository::new();
        let sid = server_id;
        repo.expect_count().returning(|_, _, _| Ok(0));
        repo.expect_get_by_key().returning(|_, _, _, _| Ok(None));
        repo.expect_upsert().returning(move |new| {
            assert!(new.expires_at.is_some());
            Ok(make_ws(sid, &new.key, new.value, new.scope, new.user_id))
        });

        let svc = make_service(repo);

        let result = svc
            .set(SetWorkspaceStateCmd {
                actor,
                server_id,
                key: "ttl.key".to_string(),
                value: serde_json::json!(42),
                scope: WorkspaceStateScopeDto::Server,
                ttl_seconds: Some(3600),
            })
            .await
            .unwrap();

        assert_eq!(result.key, "ttl.key");
    }

    #[tokio::test]
    async fn delete_returns_true_when_deleted() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let mut repo = MockWorkspaceStateRepository::new();
        repo.expect_delete_by_key().returning(|_, _, _, _| Ok(true));

        let svc = make_service(repo);

        let result = svc
            .delete(DeleteWorkspaceStateCmd {
                actor,
                server_id,
                key: "my.key".to_string(),
                scope: WorkspaceStateScopeDto::Server,
            })
            .await
            .unwrap();

        assert!(result);
    }

    #[tokio::test]
    async fn list_by_prefix() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let mut repo = MockWorkspaceStateRepository::new();
        let sid = server_id;
        repo.expect_list_by_prefix()
            .returning(move |_, _, _, _, _| {
                Ok(vec![
                    make_ws(
                        sid,
                        "app.counter",
                        serde_json::json!(1),
                        WorkspaceStateScope::Server,
                        None,
                    ),
                    make_ws(
                        sid,
                        "app.name",
                        serde_json::json!("test"),
                        WorkspaceStateScope::Server,
                        None,
                    ),
                ])
            });

        let svc = make_service(repo);

        let result = svc
            .list(ListWorkspaceStateQuery {
                actor,
                server_id,
                prefix: "app.".to_string(),
                scope: WorkspaceStateScopeDto::Server,
                limit: None,
            })
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn increment_new_key() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let mut repo = MockWorkspaceStateRepository::new();
        let sid = server_id;
        // Key doesn't exist yet — triggers quota check
        repo.expect_get_by_key().returning(|_, _, _, _| Ok(None));
        repo.expect_count().returning(|_, _, _| Ok(0));
        repo.expect_increment_by_key()
            .returning(move |_, _, _, key, delta, _| {
                Ok(make_ws(
                    sid,
                    key,
                    serde_json::json!(delta),
                    WorkspaceStateScope::Server,
                    None,
                ))
            });

        let svc = make_service(repo);

        let result = svc
            .increment(IncrementWorkspaceStateCmd {
                actor,
                server_id,
                key: "counter.views".to_string(),
                scope: WorkspaceStateScopeDto::Server,
                delta: 5,
            })
            .await
            .unwrap();

        assert_eq!(result.value, serde_json::json!(5));
    }

    #[tokio::test]
    async fn increment_existing_key() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let mut repo = MockWorkspaceStateRepository::new();
        let sid = server_id;
        // Key exists with numeric value — no quota check needed
        repo.expect_get_by_key().returning(move |_, _, _, _| {
            Ok(Some(make_ws(
                sid,
                "counter.views",
                serde_json::json!(10),
                WorkspaceStateScope::Server,
                None,
            )))
        });
        repo.expect_increment_by_key()
            .returning(move |_, _, _, key, _delta, _| {
                // Simulate existing value 10 + delta 3 = 13
                Ok(make_ws(
                    sid,
                    key,
                    serde_json::json!(13),
                    WorkspaceStateScope::Server,
                    None,
                ))
            });

        let svc = make_service(repo);

        let result = svc
            .increment(IncrementWorkspaceStateCmd {
                actor,
                server_id,
                key: "counter.views".to_string(),
                scope: WorkspaceStateScopeDto::Server,
                delta: 3,
            })
            .await
            .unwrap();

        assert_eq!(result.value, serde_json::json!(13));
    }

    #[tokio::test]
    async fn quota_enforcement_server() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let mut repo = MockWorkspaceStateRepository::new();
        repo.expect_get_by_key().returning(|_, _, _, _| Ok(None));
        repo.expect_count()
            .returning(|_, _, _| Ok(MAX_SERVER_ENTRIES));

        let svc = make_service(repo);

        let err = svc
            .set(SetWorkspaceStateCmd {
                actor,
                server_id,
                key: "new.key".to_string(),
                value: serde_json::json!("val"),
                scope: WorkspaceStateScopeDto::Server,
                ttl_seconds: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn quota_allows_upsert_existing_key() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let mut repo = MockWorkspaceStateRepository::new();
        let sid = server_id;
        // At quota limit
        repo.expect_count()
            .returning(|_, _, _| Ok(MAX_SERVER_ENTRIES));
        // But key already exists
        repo.expect_get_by_key().returning(move |_, _, _, _| {
            Ok(Some(make_ws(
                sid,
                "existing.key",
                serde_json::json!("old"),
                WorkspaceStateScope::Server,
                None,
            )))
        });
        repo.expect_upsert()
            .returning(move |new| Ok(make_ws(sid, &new.key, new.value, new.scope, new.user_id)));

        let svc = make_service(repo);

        let result = svc
            .set(SetWorkspaceStateCmd {
                actor,
                server_id,
                key: "existing.key".to_string(),
                value: serde_json::json!("updated"),
                scope: WorkspaceStateScopeDto::Server,
                ttl_seconds: None,
            })
            .await
            .unwrap();

        assert_eq!(result.value, serde_json::json!("updated"));
    }

    #[tokio::test]
    async fn key_validation_rejects_invalid() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());
        let repo = MockWorkspaceStateRepository::new();
        let svc = make_service(repo);

        let err = svc
            .set(SetWorkspaceStateCmd {
                actor,
                server_id,
                key: "INVALID".to_string(),
                value: serde_json::json!("val"),
                scope: WorkspaceStateScopeDto::Server,
                ttl_seconds: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn value_size_limit() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());
        let repo = MockWorkspaceStateRepository::new();
        let svc = make_service(repo);

        let large_value = serde_json::json!("x".repeat(65 * 1024));

        let err = svc
            .set(SetWorkspaceStateCmd {
                actor,
                server_id,
                key: "big.value".to_string(),
                value: large_value,
                scope: WorkspaceStateScopeDto::Server,
                ttl_seconds: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn negative_ttl_rejected() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());
        let repo = MockWorkspaceStateRepository::new();
        let svc = make_service(repo);

        let err = svc
            .set(SetWorkspaceStateCmd {
                actor,
                server_id,
                key: "ttl.key".to_string(),
                value: serde_json::json!("val"),
                scope: WorkspaceStateScopeDto::Server,
                ttl_seconds: Some(-10),
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn zero_ttl_rejected() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());
        let repo = MockWorkspaceStateRepository::new();
        let svc = make_service(repo);

        let err = svc
            .set(SetWorkspaceStateCmd {
                actor,
                server_id,
                key: "ttl.key".to_string(),
                value: serde_json::json!("val"),
                scope: WorkspaceStateScopeDto::Server,
                ttl_seconds: Some(0),
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn non_member_rejected() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let repo = MockWorkspaceStateRepository::new();
        let mut member_repo = MockServerMemberRepository::new();
        member_repo.expect_is_member().returning(|_, _| Ok(false));
        member_repo
            .expect_join_direct_space()
            .returning(|_, _| Ok(false));

        let svc = DefaultWorkspaceStateService::new(
            WorkspaceStateRepos {
                workspace_state_repo: Arc::new(repo),
                server_member_repo: Arc::new(member_repo),
            },
            Arc::new(rim_services::events::NoopBroadcaster),
        );

        let err = svc
            .get(GetWorkspaceStateQuery {
                actor,
                server_id,
                key: "some.key".to_string(),
                scope: WorkspaceStateScopeDto::Server,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn scope_isolation_user() {
        let server_id = ServerId::new();
        let user_id = UserId::new();
        let actor = Actor::new(user_id);

        let mut repo = MockWorkspaceStateRepository::new();
        let uid = user_id;
        repo.expect_get_by_key()
            .withf(move |_, scope, user, _| {
                *scope == WorkspaceStateScope::User && *user == Some(uid)
            })
            .returning(|_, _, _, _| Ok(None));

        let svc = make_service(repo);

        let result = svc
            .get(GetWorkspaceStateQuery {
                actor,
                server_id,
                key: "user.pref".to_string(),
                scope: WorkspaceStateScopeDto::User,
            })
            .await
            .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn system_namespace_reserved() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());
        let repo = MockWorkspaceStateRepository::new();
        let svc = make_service(repo);

        let err = svc
            .set(SetWorkspaceStateCmd {
                actor,
                server_id,
                key: "system.config".to_string(),
                value: serde_json::json!("val"),
                scope: WorkspaceStateScopeDto::Server,
                ttl_seconds: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn increment_non_numeric_rejected() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let mut repo = MockWorkspaceStateRepository::new();
        let sid = server_id;
        repo.expect_get_by_key().returning(move |_, _, _, _| {
            Ok(Some(make_ws(
                sid,
                "str.key",
                serde_json::json!("not a number"),
                WorkspaceStateScope::Server,
                None,
            )))
        });

        let svc = make_service(repo);

        let err = svc
            .increment(IncrementWorkspaceStateCmd {
                actor,
                server_id,
                key: "str.key".to_string(),
                scope: WorkspaceStateScopeDto::Server,
                delta: 1,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn increment_delta_bounds() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());
        let repo = MockWorkspaceStateRepository::new();
        let svc = make_service(repo);

        let err = svc
            .increment(IncrementWorkspaceStateCmd {
                actor,
                server_id,
                key: "counter.key".to_string(),
                scope: WorkspaceStateScopeDto::Server,
                delta: 2_000_000,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn ttl_upper_bound_rejected() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());
        let repo = MockWorkspaceStateRepository::new();
        let svc = make_service(repo);

        let err = svc
            .set(SetWorkspaceStateCmd {
                actor,
                server_id,
                key: "ttl.key".to_string(),
                value: serde_json::json!("val"),
                scope: WorkspaceStateScopeDto::Server,
                ttl_seconds: Some(MAX_TTL_SECONDS + 1),
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn increment_quota_enforced_for_new_key() {
        let server_id = ServerId::new();
        let actor = Actor::new(UserId::new());

        let mut repo = MockWorkspaceStateRepository::new();
        repo.expect_get_by_key().returning(|_, _, _, _| Ok(None));
        repo.expect_count()
            .returning(|_, _, _| Ok(MAX_SERVER_ENTRIES));

        let svc = make_service(repo);

        let err = svc
            .increment(IncrementWorkspaceStateCmd {
                actor,
                server_id,
                key: "counter.key".to_string(),
                scope: WorkspaceStateScopeDto::Server,
                delta: 1,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }
}
