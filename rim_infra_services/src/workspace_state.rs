use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use regex::Regex;

use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_core::workspace_state::model::{NewWorkspaceState, WorkspaceState, WorkspaceStateScope};
use rim_core::workspace_state::repository::WorkspaceStateRepository;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::events::{Broadcaster, WorkspaceEvent};
use rim_services::limits::InstanceLimits;
use rim_services::workspace_state::dto::{WorkspaceStateDto, WorkspaceStateScopeDto};
use rim_services::workspace_state::request::{
    DeleteWorkspaceStateCmd, GetWorkspaceStateQuery, IncrementWorkspaceStateCmd,
    ListWorkspaceStateQuery, SetWorkspaceStateCmd,
};
use rim_services::workspace_state::service::WorkspaceStateService;
use rim_services::{ServiceError, ServiceResult};

use crate::clock::Clock;
use crate::token_bucket::{BucketRate, TokenBucketLimiter};

const MAX_SERVER_ENTRIES: u64 = 10_000;
const MAX_USER_ENTRIES: u64 = 1_000;
/// Workspace-scoped entries one Member creates in a workspace; Owners and Moderators are bound
/// by the workspace's cap only.
const MAX_SERVER_ENTRIES_PER_MEMBER: u64 = 100;
/// Bytes of the values of the workspace-scoped entries one Member created in a workspace.
const MAX_SERVER_BYTES_PER_MEMBER: u64 = 1024 * 1024; // 1 MiB
const MAX_VALUE_SIZE: usize = 64 * 1024; // 64 KB
const DEFAULT_LIST_LIMIT: u64 = 100;
const MAX_LIST_LIMIT: u64 = 500;
const MAX_TTL_SECONDS: i64 = 30 * 24 * 3600; // 30 days
const MAX_PREFIX_LENGTH: usize = 512;
const MAX_INCREMENT_DELTA: i64 = 1_000_000;
const MIN_INCREMENT_DELTA: i64 = -1_000_000;
/// Writes (set, increment, delete) one user makes per minute, in either scope.
const WRITES_PER_MINUTE: u32 = 60;
/// The most writes one user makes at once before the per-minute rate applies.
const WRITE_BURST: u32 = 30;

// Any workspace member reads every key of the workspace scope and reads and writes their own
// user scope, at most WRITES_PER_MINUTE writes a minute in bursts of WRITE_BURST. Writes to the
// workspace scope take a Member, Moderator or Owner who is neither timed out nor in probation. A
// Member overwrites and deletes only the keys they created, and increments any numeric key;
// Owners and Moderators change every key. Writes are bounded by the entry counts, bytes, value
// size, TTL and increment limits above.

#[derive(Clone)]
pub struct WorkspaceStateRepos {
    pub workspace_state_repo: Arc<dyn WorkspaceStateRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    /// Reads a writer's account age for probation.
    pub user_repo: Arc<dyn UserRepository>,
}

#[derive(Clone)]
pub struct DefaultWorkspaceStateService {
    repos: WorkspaceStateRepos,
    broadcaster: Arc<dyn Broadcaster>,
    /// The relay's direct space, which has no workspace state.
    direct_space_id: Option<ServerId>,
    /// The probation of new accounts.
    limits: InstanceLimits,
    clock: Clock,
    /// Each user's writes, whichever path (GraphQL, commands, macros, pipelines) makes them.
    write_budget: Arc<TokenBucketLimiter<UserId>>,
}

impl DefaultWorkspaceStateService {
    pub fn new(repos: WorkspaceStateRepos, broadcaster: Arc<dyn Broadcaster>) -> Self {
        Self {
            repos,
            broadcaster,
            direct_space_id: None,
            limits: InstanceLimits::default(),
            clock: Clock::default(),
            write_budget: Arc::new(TokenBucketLimiter::new(BucketRate {
                per_minute: WRITES_PER_MINUTE,
                burst: WRITE_BURST,
            })),
        }
    }

    /// `RateLimited` once `user_id` has used their write budget.
    fn ensure_write_budget(&self, user_id: UserId) -> ServiceResult<()> {
        match self.write_budget.check(&user_id, std::time::Instant::now()) {
            RateLimitDecision::Allowed => Ok(()),
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => {
                crate::metrics::rate_limited("workspace_state_writes");
                Err(ServiceError::RateLimited {
                    retry_after_seconds,
                })
            }
        }
    }

    /// The relay's direct space, or `None` where none is wired.
    pub fn with_direct_space(mut self, direct_space_id: Option<ServerId>) -> Self {
        self.direct_space_id = direct_space_id;
        self
    }

    pub fn with_limits(mut self, limits: InstanceLimits) -> Self {
        self.limits = limits;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    fn validate_key(key: &str) -> ServiceResult<()> {
        static KEY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^[a-z][a-z0-9_.:-]{0,510}[a-z0-9]$").expect("valid regex")
        });

        if !KEY_REGEX.is_match(key) {
            return Err(ServiceError::Validation(format!(
                "invalid key format: must match ^[a-z][a-z0-9_.:-]{{0,510}}[a-z0-9]$, got `{}`",
                key.chars().take(64).collect::<String>()
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

    /// The bytes of `value` as compact JSON text.
    fn value_size(value: &serde_json::Value) -> usize {
        serde_json::to_vec(value).map(|v| v.len()).unwrap_or(0)
    }

    fn validate_value_size(value: &serde_json::Value) -> ServiceResult<()> {
        let size = Self::value_size(value);
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

    /// The role of a writer of `scope` in `server_id`: `None` for the user scope, which every
    /// member writes; for the workspace scope the writer's role, once they are shown to be a
    /// member who is no guest, not timed out and not in probation.
    ///
    /// # Errors
    /// `Validation` in the direct space or while the account is in probation (an instance
    /// operator never is),
    /// `PermissionDenied` for a non-member or a guest, `TimedOut` while timed out.
    async fn ensure_writer(
        &self,
        server_id: ServerId,
        user_id: UserId,
        scope: WorkspaceStateScope,
    ) -> ServiceResult<Option<ServerMemberRole>> {
        if scope == WorkspaceStateScope::User {
            self.ensure_member(server_id, user_id).await?;
            return Ok(None);
        }
        crate::util::ensure_workspace_features(self.direct_space_id, server_id)?;
        let standing = crate::util::ensure_workspace_writable(
            self.repos.server_member_repo.as_ref(),
            server_id,
            user_id,
        )
        .await?;
        if standing.role.is_guest() {
            return Err(ServiceError::PermissionDenied);
        }
        if self.limits.probation_hours.is_some() {
            let user = self
                .repos
                .user_repo
                .get_by_id(user_id)
                .await?
                .ok_or(ServiceError::PermissionDenied)?;
            crate::probation::ensure_out_of_probation(
                &self.limits,
                self.repos.user_repo.as_ref(),
                &user,
                self.clock.now(),
                "change workspace state",
            )
            .await?;
        }
        Ok(Some(standing.role))
    }

    /// `Forbidden` when a writer of `role` may not overwrite or delete `existing`: a Member
    /// changes only the workspace-scoped entries they created.
    fn ensure_may_change(
        role: Option<ServerMemberRole>,
        existing: Option<&WorkspaceState>,
        user_id: UserId,
    ) -> ServiceResult<()> {
        match (role, existing) {
            (Some(role), Some(entry)) if !role.is_moderator() && entry.created_by != user_id => {
                Err(ServiceError::Forbidden(
                    "only the member who created this key, an owner or a moderator can change it"
                        .to_string(),
                ))
            }
            _ => Ok(()),
        }
    }

    /// `Validation` when a new entry of `scope` would pass the scope's cap, or, for a Member
    /// writing the workspace scope, their own cap on entries; a new entry is one whose key is
    /// not stored yet.
    async fn ensure_room_for_new_entry(
        &self,
        server_id: ServerId,
        scope: WorkspaceStateScope,
        user_id: Option<UserId>,
        role: Option<ServerMemberRole>,
        writer: UserId,
    ) -> ServiceResult<()> {
        let count = self
            .repos
            .workspace_state_repo
            .count(server_id, scope, user_id)
            .await?;
        let max = match scope {
            WorkspaceStateScope::Server => MAX_SERVER_ENTRIES,
            WorkspaceStateScope::User => MAX_USER_ENTRIES,
        };
        if count >= max {
            return Err(ServiceError::Validation(format!(
                "quota exceeded: maximum {max} entries for {scope} scope",
                scope = scope.as_str()
            )));
        }
        if role.is_some_and(|r| !r.is_moderator()) {
            let usage = self
                .repos
                .workspace_state_repo
                .creator_usage(server_id, writer)
                .await?;
            if usage.entries >= MAX_SERVER_ENTRIES_PER_MEMBER {
                return Err(ServiceError::Validation(format!(
                    "quota exceeded: a member creates at most {MAX_SERVER_ENTRIES_PER_MEMBER} \
                     workspace-scoped entries"
                )));
            }
        }
        Ok(())
    }

    /// `Validation` when a Member's workspace-scoped values would hold more than their byte cap
    /// once `value` replaces `existing`, which counts only when they created it.
    async fn ensure_room_for_bytes(
        &self,
        server_id: ServerId,
        writer: UserId,
        existing: Option<&WorkspaceState>,
        value: &serde_json::Value,
    ) -> ServiceResult<()> {
        let usage = self
            .repos
            .workspace_state_repo
            .creator_usage(server_id, writer)
            .await?;
        let replaced = existing
            .filter(|e| e.created_by == writer)
            .map_or(0, |e| Self::value_size(&e.value) as u64);
        let total = usage.bytes.saturating_sub(replaced) + Self::value_size(value) as u64;
        if total > MAX_SERVER_BYTES_PER_MEMBER {
            return Err(ServiceError::Validation(format!(
                "quota exceeded: a member's workspace-scoped values hold at most \
                 {MAX_SERVER_BYTES_PER_MEMBER} bytes"
            )));
        }
        Ok(())
    }

    /// Tells the workspace's members, or for the user scope its one user, that a key changed.
    /// The value is left out: a client that shows it reads it again.
    fn broadcast_changed(&self, server_id: ServerId, dto: &WorkspaceStateDto) {
        self.broadcaster
            .broadcast(WorkspaceEvent::WorkspaceStateChanged {
                workspace_id: server_id,
                key: dto.key.clone(),
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
        let (scope, user_id) = Self::resolve_scope(cmd.scope, cmd.actor.user_id);
        let role = self
            .ensure_writer(cmd.server_id, cmd.actor.user_id, scope)
            .await?;
        self.ensure_write_budget(cmd.actor.user_id)?;
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

        let existing = self
            .repos
            .workspace_state_repo
            .get_by_key(cmd.server_id, scope, user_id, &cmd.key)
            .await?;
        Self::ensure_may_change(role, existing.as_ref(), cmd.actor.user_id)?;
        if existing.is_none() {
            self.ensure_room_for_new_entry(cmd.server_id, scope, user_id, role, cmd.actor.user_id)
                .await?;
        }
        if role.is_some_and(|r| !r.is_moderator()) {
            self.ensure_room_for_bytes(
                cmd.server_id,
                cmd.actor.user_id,
                existing.as_ref(),
                &cmd.value,
            )
            .await?;
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
        let (scope, user_id) = Self::resolve_scope(cmd.scope, cmd.actor.user_id);
        let role = self
            .ensure_writer(cmd.server_id, cmd.actor.user_id, scope)
            .await?;
        self.ensure_write_budget(cmd.actor.user_id)?;
        if role.is_some_and(|r| !r.is_moderator()) {
            let existing = self
                .repos
                .workspace_state_repo
                .get_by_key(cmd.server_id, scope, user_id, &cmd.key)
                .await?;
            Self::ensure_may_change(role, existing.as_ref(), cmd.actor.user_id)?;
        }
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
        let (scope, user_id) = Self::resolve_scope(cmd.scope, cmd.actor.user_id);
        let role = self
            .ensure_writer(cmd.server_id, cmd.actor.user_id, scope)
            .await?;
        self.ensure_write_budget(cmd.actor.user_id)?;
        Self::validate_key(&cmd.key)?;

        if cmd.delta < MIN_INCREMENT_DELTA || cmd.delta > MAX_INCREMENT_DELTA {
            return Err(ServiceError::Validation(format!(
                "delta must be between {MIN_INCREMENT_DELTA} and {MAX_INCREMENT_DELTA}"
            )));
        }

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
            self.ensure_room_for_new_entry(cmd.server_id, scope, user_id, role, cmd.actor.user_id)
                .await?;
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
    use rim_core::server_member::model::{MemberStanding, ServerMember, ServerMemberRole};
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
            async fn creator_usage(
                &self,
                server_id: ServerId,
                creator: UserId,
            ) -> RepoResult<rim_core::workspace_state::model::WorkspaceStateUsage>;
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
            async fn count_members(&self, server_id: rim_core::server::model::ServerId) -> RepoResult<u64>;
            async fn get_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<Option<ServerMember>>;
            async fn add_member(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<ServerMember>;
            async fn change_member_role(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<()>;
            async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()>;
            async fn get_members_for_server(&self, server_id: ServerId) -> RepoResult<Vec<ServerMember>>;
            async fn get_members(&self, server_id: ServerId, user_ids: &[UserId]) -> RepoResult<Vec<ServerMember>>;
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn join_direct_space(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn get_member_standing(&self, server_id: rim_core::server::model::ServerId, user_id: rim_core::user::model::UserId) -> RepoResult<Option<rim_core::server_member::model::MemberStanding>>;
            async fn get_owner_id(&self, server_id: rim_core::server::model::ServerId) -> RepoResult<Option<rim_core::user::model::UserId>>;
        }
    }

    /// A member repository where everyone is a member with `role` and no timeout.
    fn make_member_repo(role: ServerMemberRole) -> MockServerMemberRepository {
        let mut repo = MockServerMemberRepository::new();
        repo.expect_is_member().returning(|_, _| Ok(true));
        repo.expect_get_member_standing()
            .returning(move |_, _| Ok(Some(MemberStanding::new(role))));
        repo
    }

    /// The service as the workspace's Owner sees it.
    fn make_service(repo: MockWorkspaceStateRepository) -> DefaultWorkspaceStateService {
        make_service_as(repo, ServerMemberRole::Owner)
    }

    /// The service where every caller has `role`.
    fn make_service_as(
        repo: MockWorkspaceStateRepository,
        role: ServerMemberRole,
    ) -> DefaultWorkspaceStateService {
        DefaultWorkspaceStateService::new(
            WorkspaceStateRepos {
                workspace_state_repo: Arc::new(repo),
                server_member_repo: Arc::new(make_member_repo(role)),
                user_repo: Arc::new(rim_core::user::repository::MockUserRepository::new()),
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

    /// 63 ASCII bytes followed by `é`, whose two bytes straddle byte 64.
    fn key_split_at_preview_boundary() -> String {
        format!("{}é", "a".repeat(63))
    }

    #[test]
    fn key_validation_previews_whole_characters() {
        for key in [
            key_split_at_preview_boundary(),
            format!("{}€", "a".repeat(62)),
            format!("{}😀", "a".repeat(61)),
            "é".repeat(100),
        ] {
            let err = DefaultWorkspaceStateService::validate_key(&key).unwrap_err();
            let ServiceError::Validation(message) = err else {
                panic!("expected a validation error for {key:?}, got {err:?}");
            };
            let preview: String = key.chars().take(64).collect();
            assert!(message.ends_with(&format!("got `{preview}`")), "{message}");
        }
    }

    #[tokio::test]
    async fn set_rejects_non_ascii_key_at_preview_boundary() {
        let svc = make_service(MockWorkspaceStateRepository::new());

        let err = svc
            .set(SetWorkspaceStateCmd {
                actor: Actor::new(UserId::new()),
                server_id: ServerId::new(),
                key: key_split_at_preview_boundary(),
                value: serde_json::json!("val"),
                scope: WorkspaceStateScopeDto::Server,
                ttl_seconds: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn increment_rejects_non_ascii_key_at_preview_boundary() {
        let svc = make_service(MockWorkspaceStateRepository::new());

        let err = svc
            .increment(IncrementWorkspaceStateCmd {
                actor: Actor::new(UserId::new()),
                server_id: ServerId::new(),
                key: key_split_at_preview_boundary(),
                scope: WorkspaceStateScopeDto::Server,
                delta: 1,
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
                user_repo: Arc::new(rim_core::user::repository::MockUserRepository::new()),
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

    /// An entry of `key` in the workspace scope, created by `creator`.
    fn entry_by(
        server_id: ServerId,
        key: &str,
        value: serde_json::Value,
        creator: UserId,
    ) -> WorkspaceState {
        WorkspaceState {
            created_by: creator,
            ..make_ws(server_id, key, value, WorkspaceStateScope::Server, None)
        }
    }

    fn set_cmd(actor: UserId, server_id: ServerId, key: &str) -> SetWorkspaceStateCmd {
        SetWorkspaceStateCmd {
            actor: Actor::new(actor),
            server_id,
            key: key.to_string(),
            value: serde_json::json!("v"),
            scope: WorkspaceStateScopeDto::Server,
            ttl_seconds: None,
        }
    }

    fn delete_cmd(actor: UserId, server_id: ServerId, key: &str) -> DeleteWorkspaceStateCmd {
        DeleteWorkspaceStateCmd {
            actor: Actor::new(actor),
            server_id,
            key: key.to_string(),
            scope: WorkspaceStateScopeDto::Server,
        }
    }

    fn increment_cmd(actor: UserId, server_id: ServerId, key: &str) -> IncrementWorkspaceStateCmd {
        IncrementWorkspaceStateCmd {
            actor: Actor::new(actor),
            server_id,
            key: key.to_string(),
            scope: WorkspaceStateScopeDto::Server,
            delta: 1,
        }
    }

    fn usage(entries: u64, bytes: u64) -> rim_core::workspace_state::model::WorkspaceStateUsage {
        rim_core::workspace_state::model::WorkspaceStateUsage { entries, bytes }
    }

    #[tokio::test]
    async fn a_guest_writes_only_their_own_user_scope() {
        let server_id = ServerId::new();
        let guest = UserId::new();
        let mut repo = MockWorkspaceStateRepository::new();
        repo.expect_get_by_key().returning(|_, _, _, _| Ok(None));
        repo.expect_count().returning(|_, _, _| Ok(0));
        repo.expect_upsert().returning(move |new| {
            Ok(make_ws(
                server_id,
                &new.key,
                new.value,
                new.scope,
                new.user_id,
            ))
        });
        let svc = make_service_as(repo, ServerMemberRole::Guest);

        for err in [
            svc.set(set_cmd(guest, server_id, "board.x"))
                .await
                .unwrap_err(),
            svc.delete(delete_cmd(guest, server_id, "board.x"))
                .await
                .unwrap_err(),
            svc.increment(increment_cmd(guest, server_id, "board.x"))
                .await
                .unwrap_err(),
        ] {
            assert!(matches!(err, ServiceError::PermissionDenied), "{err:?}");
        }

        let own = svc
            .set(SetWorkspaceStateCmd {
                scope: WorkspaceStateScopeDto::User,
                ..set_cmd(guest, server_id, "prefs.x")
            })
            .await
            .unwrap();
        assert_eq!(own.user_id, Some(guest));
    }

    #[tokio::test]
    async fn an_account_in_probation_cannot_write_the_workspace_scope() {
        let server_id = ServerId::new();
        let newcomer = UserId::new();
        let now = chrono::Utc::now();
        let svc = DefaultWorkspaceStateService::new(
            WorkspaceStateRepos {
                workspace_state_repo: Arc::new(MockWorkspaceStateRepository::new()),
                server_member_repo: Arc::new(make_member_repo(ServerMemberRole::Member)),
                user_repo: Arc::new(crate::probation_fixtures::users(vec![
                    crate::probation_fixtures::account(
                        newcomer,
                        now - chrono::Duration::hours(1),
                        None,
                    ),
                ])),
            },
            Arc::new(rim_services::events::NoopBroadcaster),
        )
        .with_limits(InstanceLimits {
            probation_hours: Some(24),
            ..Default::default()
        })
        .with_clock(Clock::fixed(now));

        let err = svc
            .set(set_cmd(newcomer, server_id, "board.x"))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m == "a new account can change workspace state in 1380 minutes"),
            "{err:?}"
        );
        let err = svc
            .increment(increment_cmd(newcomer, server_id, "board.x"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
    }

    #[tokio::test]
    async fn a_timed_out_member_cannot_write_the_workspace_scope() {
        let mut member_repo = MockServerMemberRepository::new();
        member_repo.expect_get_member_standing().returning(|_, _| {
            Ok(Some(MemberStanding {
                role: ServerMemberRole::Member,
                timed_out_until: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            }))
        });
        let svc = DefaultWorkspaceStateService::new(
            WorkspaceStateRepos {
                workspace_state_repo: Arc::new(MockWorkspaceStateRepository::new()),
                server_member_repo: Arc::new(member_repo),
                user_repo: Arc::new(rim_core::user::repository::MockUserRepository::new()),
            },
            Arc::new(rim_services::events::NoopBroadcaster),
        );

        let err = svc
            .set(set_cmd(UserId::new(), ServerId::new(), "board.x"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::TimedOut { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn a_member_changes_only_the_keys_they_created() {
        let server_id = ServerId::new();
        let member = UserId::new();
        let other = UserId::new();
        let mut repo = MockWorkspaceStateRepository::new();
        repo.expect_get_by_key().returning(move |_, _, _, key| {
            let creator = if key == "board.mine" { member } else { other };
            Ok(Some(entry_by(
                server_id,
                key,
                serde_json::json!("x"),
                creator,
            )))
        });
        repo.expect_creator_usage()
            .returning(|_, _| Ok(usage(1, 3)));
        repo.expect_upsert().returning(move |new| {
            Ok(make_ws(
                server_id,
                &new.key,
                new.value,
                new.scope,
                new.user_id,
            ))
        });
        repo.expect_delete_by_key().returning(|_, _, _, _| Ok(true));
        let svc = make_service_as(repo, ServerMemberRole::Member);

        let err = svc
            .set(set_cmd(member, server_id, "board.theirs"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Forbidden(_)), "{err:?}");
        let err = svc
            .delete(delete_cmd(member, server_id, "board.theirs"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Forbidden(_)), "{err:?}");

        svc.set(set_cmd(member, server_id, "board.mine"))
            .await
            .unwrap();
        assert!(
            svc.delete(delete_cmd(member, server_id, "board.mine"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_member_increments_a_counter_someone_else_created() {
        let server_id = ServerId::new();
        let mut repo = MockWorkspaceStateRepository::new();
        repo.expect_get_by_key().returning(move |_, _, _, key| {
            Ok(Some(entry_by(
                server_id,
                key,
                serde_json::json!(4),
                UserId::new(),
            )))
        });
        repo.expect_increment_by_key()
            .returning(move |_, _, _, key, _, _| {
                Ok(entry_by(
                    server_id,
                    key,
                    serde_json::json!(5),
                    UserId::new(),
                ))
            });
        let svc = make_service_as(repo, ServerMemberRole::Member);

        let entry = svc
            .increment(increment_cmd(UserId::new(), server_id, "poll.yes"))
            .await
            .unwrap();
        assert_eq!(entry.value, serde_json::json!(5));
    }

    #[tokio::test]
    async fn owners_and_moderators_change_every_key_without_a_per_member_cap() {
        for role in [ServerMemberRole::Owner, ServerMemberRole::Moderator] {
            let server_id = ServerId::new();
            let mut repo = MockWorkspaceStateRepository::new();
            repo.expect_get_by_key().returning(move |_, _, _, key| {
                if key == "board.new" {
                    return Ok(None);
                }
                Ok(Some(entry_by(
                    server_id,
                    key,
                    serde_json::json!("x"),
                    UserId::new(),
                )))
            });
            repo.expect_count().returning(|_, _, _| Ok(0));
            repo.expect_creator_usage().never();
            repo.expect_upsert().returning(move |new| {
                Ok(make_ws(
                    server_id,
                    &new.key,
                    new.value,
                    new.scope,
                    new.user_id,
                ))
            });
            repo.expect_delete_by_key().returning(|_, _, _, _| Ok(true));
            let svc = make_service_as(repo, role);
            let actor = UserId::new();

            svc.set(set_cmd(actor, server_id, "board.theirs"))
                .await
                .unwrap();
            svc.set(set_cmd(actor, server_id, "board.new"))
                .await
                .unwrap();
            assert!(
                svc.delete(delete_cmd(actor, server_id, "board.theirs"))
                    .await
                    .unwrap()
            );
        }
    }

    #[tokio::test]
    async fn a_member_creates_at_most_a_hundred_workspace_scoped_entries() {
        let server_id = ServerId::new();
        let mut repo = MockWorkspaceStateRepository::new();
        repo.expect_get_by_key().returning(|_, _, _, _| Ok(None));
        repo.expect_count().returning(|_, _, _| Ok(500));
        repo.expect_creator_usage()
            .returning(|_, _| Ok(usage(MAX_SERVER_ENTRIES_PER_MEMBER, 10)));
        let svc = make_service_as(repo, ServerMemberRole::Member);
        let member = UserId::new();

        for err in [
            svc.set(set_cmd(member, server_id, "board.new"))
                .await
                .unwrap_err(),
            svc.increment(increment_cmd(member, server_id, "board.new"))
                .await
                .unwrap_err(),
        ] {
            assert!(
                matches!(&err, ServiceError::Validation(m) if m.contains("at most 100")),
                "{err:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_members_values_hold_at_most_a_mebibyte() {
        let server_id = ServerId::new();
        let member = UserId::new();
        let mut repo = MockWorkspaceStateRepository::new();
        repo.expect_get_by_key().returning(move |_, _, _, key| {
            if key == "board.mine" {
                // `"0123456789"` is 12 bytes of JSON text.
                return Ok(Some(entry_by(
                    server_id,
                    key,
                    serde_json::json!("0123456789"),
                    member,
                )));
            }
            Ok(None)
        });
        repo.expect_count().returning(|_, _, _| Ok(0));
        repo.expect_creator_usage()
            .returning(|_, _| Ok(usage(5, MAX_SERVER_BYTES_PER_MEMBER - 10)));
        repo.expect_upsert().returning(move |new| {
            Ok(make_ws(
                server_id,
                &new.key,
                new.value,
                new.scope,
                new.user_id,
            ))
        });
        let svc = make_service_as(repo, ServerMemberRole::Member);
        let with_value = |key: &str, value: serde_json::Value| SetWorkspaceStateCmd {
            value,
            ..set_cmd(member, server_id, key)
        };

        // 10 bytes are left: a new 12-byte value does not fit, a 10-byte one does.
        let err = svc
            .set(with_value("board.new", serde_json::json!("0123456789")))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("1048576 bytes")),
            "{err:?}"
        );
        svc.set(with_value("board.new", serde_json::json!("01234567")))
            .await
            .unwrap();
        // Replacing their own 12-byte value frees its bytes: 22 bytes fit.
        svc.set(with_value(
            "board.mine",
            serde_json::json!("01234567890123456789"),
        ))
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_user_makes_at_most_a_burst_of_writes_at_once() {
        let server_id = ServerId::new();
        let mut repo = MockWorkspaceStateRepository::new();
        repo.expect_get_by_key().returning(|_, _, _, _| Ok(None));
        repo.expect_count().returning(|_, _, _| Ok(0));
        repo.expect_upsert().returning(move |new| {
            Ok(make_ws(
                server_id,
                &new.key,
                new.value,
                new.scope,
                new.user_id,
            ))
        });
        repo.expect_delete_by_key()
            .returning(|_, _, _, _| Ok(false));
        let svc = make_service(repo);
        let (writer, other) = (UserId::new(), UserId::new());
        for i in 0..WRITE_BURST {
            svc.set(set_cmd(writer, server_id, &format!("burst.k{i}")))
                .await
                .expect("within the burst");
        }
        for err in [
            svc.set(set_cmd(writer, server_id, "burst.more"))
                .await
                .unwrap_err(),
            svc.delete(delete_cmd(writer, server_id, "burst.k0"))
                .await
                .unwrap_err(),
        ] {
            assert!(matches!(err, ServiceError::RateLimited { .. }), "{err:?}");
        }
        svc.set(set_cmd(other, server_id, "burst.other"))
            .await
            .expect("another user's budget is their own");
    }
}
