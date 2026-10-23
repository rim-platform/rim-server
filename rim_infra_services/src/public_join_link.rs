//! Workspaces' public join links and the join requests filed through them.
//!
//! [`DefaultPublicJoinLinkService`] lets a workspace's Owner turn a permanent link on and off,
//! issue a new token and choose what it does; lets anyone with the link see the workspace and an
//! account of this server join or ask to join; and lets the Owner and Moderators accept or
//! decline the requests. The link never creates an account. Every invalid link answers
//! `NotFound` on one path, a lookup by the token's hash, and [`LinkMissLimiter`] cuts off an
//! address that makes too many such lookups.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use rim_infra_db::{TransactionScope, begin_transaction};
use sea_orm::DatabaseConnection;
use tracing::{info, warn};

use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::moderation::repository::ModerationRepository;
use rim_core::public_join_link::model::{
    JoinRequest, JoinRequestId, JoinRequestStatus, NewJoinRequest, PublicJoinLink, PublicLinkMode,
};
use rim_core::public_join_link::repository::{ListJoinRequestsParams, PublicJoinLinkRepository};
use rim_core::repository::pagination::Page;
use rim_core::server::model::{Server, ServerId};
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_services::audit::service::AuditLogService;
use rim_services::command::verb::TypedVerb;
use rim_services::default_workspace::DefaultWorkspaceService;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::limits::{InstanceLimits, ensure_below};
use rim_services::public_join_link::dto::{
    JOIN_REQUEST_DAYS, JOIN_REQUEST_RETRY_HOURS, JoinByPublicLinkOutcome,
    JoinByPublicLinkResultDto, JoinRequestDto, JoinRequestPageCursor, LINK_MISS_BLOCK_SECS,
    LINK_MISS_LIMIT, LINK_MISS_WINDOW_SECS, MAX_JOIN_REQUEST_MESSAGE_CHARS,
    MAX_OPEN_JOIN_REQUESTS_PER_USER, PUBLIC_LINK_SOURCE, PublicJoinLinkDto, PublicJoinLinkPatch,
    PublicJoinLinkPreviewDto, PublicLinkViewer,
};
use rim_services::public_join_link::service::{ListJoinRequestsQuery, PublicJoinLinkService};
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::clock::Clock;
use crate::link_key::LinkKey;
use crate::moderation::{ModerationAuditEntry, record_audit_action};
use crate::repos::AppRepos;
use crate::util::{add_workspace_member, ensure_not_banned, ensure_workspace_features};

/// `initiator` of the audit rows the link and join requests write.
const AUDIT_INITIATOR: &str = "public_link";

/// Pseudo command ids under which link changes, join decisions and joins through a link are
/// recorded in `command_audit_log`.
pub(crate) mod audit_ids {
    pub(crate) const ENABLED: &str = "public_link.enabled";
    pub(crate) const DISABLED: &str = "public_link.disabled";
    pub(crate) const REISSUED: &str = "public_link.reissued";
    pub(crate) const SETTINGS_CHANGED: &str = "public_link.settings_changed";
    pub(crate) const REQUEST_ACCEPTED: &str = "join_request.accepted";
    pub(crate) const REQUEST_DECLINED: &str = "join_request.declined";
    /// A member joined through the link, directly or by an accepted request.
    pub(crate) const MEMBER_JOINED: &str = "workspace.member_joined";
}

/// Most addresses [`LinkMissLimiter`] keeps; the oldest goes first beyond that.
const MAX_MISS_BUCKETS: usize = 100_000;

/// The repositories the link service reads and writes.
#[derive(Clone)]
pub struct PublicJoinLinkRepos {
    pub link_repo: Arc<dyn PublicJoinLinkRepository>,
    pub server_repo: Arc<dyn ServerRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    /// A direct join and an accepted request join the public channels.
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub moderation_repo: Arc<dyn ModerationRepository>,
    /// The joiner's username for `MemberJoined`.
    pub user_repo: Arc<dyn UserRepository>,
}

#[derive(Default)]
struct MissBucket {
    misses: VecDeque<DateTime<Utc>>,
    blocked_until: Option<DateTime<Utc>>,
}

/// Failed link lookups and joins per client address: [`LINK_MISS_LIMIT`] of them within
/// [`LINK_MISS_WINDOW_SECS`] cut the address off for [`LINK_MISS_BLOCK_SECS`]. Successful
/// lookups are not counted. Single-node and in memory.
#[derive(Default)]
pub(crate) struct LinkMissLimiter {
    buckets: DashMap<String, MissBucket>,
    order: Mutex<VecDeque<String>>,
}

impl LinkMissLimiter {
    /// `RateLimited`, with the seconds left, while `address` is cut off at `now`.
    fn check(&self, address: &str, now: DateTime<Utc>) -> ServiceResult<()> {
        let Some(bucket) = self.buckets.get(address) else {
            return Ok(());
        };
        match bucket.blocked_until {
            Some(until) if until > now => Err(ServiceError::RateLimited {
                retry_after_seconds: u64::try_from((until - now).num_seconds().max(1)).unwrap_or(1),
            }),
            _ => Ok(()),
        }
    }

    /// Count a miss from `address` at `now`, and cut the address off once it reaches the limit
    /// within the window.
    fn record(&self, address: &str, now: DateTime<Utc>) {
        let is_new = !self.buckets.contains_key(address);
        {
            let mut bucket = self.buckets.entry(address.to_string()).or_default();
            let window_start = now - Duration::seconds(LINK_MISS_WINDOW_SECS);
            while bucket.misses.front().is_some_and(|at| *at <= window_start) {
                bucket.misses.pop_front();
            }
            bucket.misses.push_back(now);
            if bucket.misses.len() >= LINK_MISS_LIMIT as usize {
                bucket.blocked_until = Some(now + Duration::seconds(LINK_MISS_BLOCK_SECS));
                bucket.misses.clear();
            }
        }
        if is_new && let Ok(mut order) = self.order.lock() {
            order.push_back(address.to_string());
            while order.len() > MAX_MISS_BUCKETS {
                if let Some(oldest) = order.pop_front() {
                    self.buckets.remove(&oldest);
                }
            }
        }
    }
}

/// The workspace's Owner and Moderators, and `person`: who a join request event reaches.
async fn request_audience(
    server_member_repo: &dyn ServerMemberRepository,
    server_id: ServerId,
    person: UserId,
) -> ServiceResult<Vec<UserId>> {
    let mut audience: Vec<UserId> = server_member_repo
        .get_members_for_server(server_id)
        .await?
        .into_iter()
        .filter(|m| m.member_role.is_moderator())
        .map(|m| m.user_id)
        .collect();
    if !audience.contains(&person) {
        audience.push(person);
    }
    Ok(audience)
}

/// Announce that `request` was closed with `outcome` to the workspace's Owner and Moderators and
/// to its person. A failure to read the audience is logged; the request is closed already.
async fn announce_decided(
    server_member_repo: &dyn ServerMemberRepository,
    observers: &Observers,
    request: &JoinRequest,
    outcome: JoinRequestStatus,
) {
    match request_audience(server_member_repo, request.server_id, request.user_id).await {
        Ok(member_user_ids) => observers.emit(WorkspaceEvent::JoinRequestDecided {
            workspace_id: request.server_id,
            request_id: request.id,
            user_id: request.user_id,
            outcome: outcome.as_str().to_string(),
            member_user_ids,
        }),
        Err(e) => {
            warn!(error = %e, request_id = %request.id, "join request decision not announced")
        }
    }
}

/// Cancel the open join requests of a person just banned from the workspace, and announce each.
///
/// # Errors
/// Storage errors cancelling the requests.
pub(crate) async fn cancel_requests_on_ban(
    link_repo: &dyn PublicJoinLinkRepository,
    server_member_repo: &dyn ServerMemberRepository,
    observers: &Observers,
    server_id: ServerId,
    user_id: UserId,
) -> ServiceResult<()> {
    for request in link_repo
        .cancel_open_requests(server_id, user_id, Utc::now())
        .await?
    {
        announce_decided(
            server_member_repo,
            observers,
            &request,
            JoinRequestStatus::Cancelled,
        )
        .await;
    }
    Ok(())
}

/// The trimmed message of a join request; `None` when empty.
///
/// # Errors
/// `Validation` beyond [`MAX_JOIN_REQUEST_MESSAGE_CHARS`] characters.
fn normalize_message(message: Option<String>) -> ServiceResult<Option<String>> {
    let Some(message) = message.map(|m| m.trim().to_string()) else {
        return Ok(None);
    };
    if message.is_empty() {
        return Ok(None);
    }
    if message.chars().count() > MAX_JOIN_REQUEST_MESSAGE_CHARS {
        return Err(ServiceError::Validation(format!(
            "a join request message is at most {MAX_JOIN_REQUEST_MESSAGE_CHARS} characters"
        )));
    }
    Ok(Some(message))
}

pub struct DefaultPublicJoinLinkService {
    repos: PublicJoinLinkRepos,
    db: Option<DatabaseConnection>,
    observers: Observers,
    /// `None` in unit tests; the composition root always wires it.
    audit_log_service: Option<Arc<dyn AuditLogService>>,
    /// `None` where the server has no link key: no link can be issued or shown.
    link_key: Option<LinkKey>,
    /// What a link's URL starts with: `<link base>/join/<token>`.
    link_base: String,
    /// The direct space, which has no link. `None` where no direct space is wired.
    direct_space_id: Option<ServerId>,
    /// The server's default workspace, which has no link: every account joins it anyway.
    default_workspace: Option<Arc<dyn DefaultWorkspaceService>>,
    limits: InstanceLimits,
    misses: Arc<LinkMissLimiter>,
    clock: Clock,
}

impl DefaultPublicJoinLinkService {
    /// A service sealing tokens under `link_key`, building links on `link_base`.
    pub fn new(
        repos: PublicJoinLinkRepos,
        observers: Observers,
        link_key: Option<LinkKey>,
        link_base: String,
    ) -> Self {
        Self {
            repos,
            db: None,
            observers,
            audit_log_service: None,
            link_key,
            link_base: link_base.trim_end_matches('/').to_string(),
            direct_space_id: None,
            default_workspace: None,
            limits: InstanceLimits::default(),
            misses: Arc::default(),
            clock: Clock::default(),
        }
    }

    /// Run each join and decision in one transaction on `db`.
    pub fn with_database(mut self, db: DatabaseConnection) -> Self {
        self.db = Some(db);
        self
    }

    pub fn with_audit_log_service(mut self, audit_log_service: Arc<dyn AuditLogService>) -> Self {
        self.audit_log_service = Some(audit_log_service);
        self
    }

    pub fn with_direct_space(mut self, direct_space_id: Option<ServerId>) -> Self {
        self.direct_space_id = direct_space_id;
        self
    }

    pub fn with_default_workspace(mut self, service: Arc<dyn DefaultWorkspaceService>) -> Self {
        self.default_workspace = Some(service);
        self
    }

    /// Apply the instance's member cap to joins.
    pub fn with_limits(mut self, limits: InstanceLimits) -> Self {
        self.limits = limits;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    async fn transaction(&self) -> ServiceResult<(Option<TransactionScope>, PublicJoinLinkRepos)> {
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_public_join_link(),
            None => self.repos.clone(),
        };
        Ok((scope, repos))
    }

    async fn commit(scope: Option<TransactionScope>) -> ServiceResult<()> {
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        Ok(())
    }

    async fn audit(
        &self,
        actor: UserId,
        server_id: ServerId,
        id: &'static str,
        args: serde_json::Value,
    ) {
        record_audit_action(
            self.audit_log_service.as_ref(),
            ModerationAuditEntry {
                actor,
                server_id,
                command_id: id,
                args,
                dangerous: false,
                typed_verb: TypedVerb::Configure,
            },
            AUDIT_INITIATOR,
        )
        .await;
    }

    /// The workspace's link for its Owner, after refusing the direct space, anyone but the
    /// Owner, and the server's default workspace.
    async fn owner_link(
        &self,
        actor: Actor,
        server_id: ServerId,
    ) -> ServiceResult<Option<PublicJoinLink>> {
        ensure_workspace_features(self.direct_space_id, server_id)?;
        let member = self
            .repos
            .server_member_repo
            .get_member(server_id, actor.user_id)
            .await?
            .ok_or(ServiceError::PermissionDenied)?;
        if member.member_role != ServerMemberRole::Owner {
            return Err(ServiceError::PermissionDenied);
        }
        if let Some(default_workspace) = &self.default_workspace
            && default_workspace.current().await? == Some(server_id)
        {
            return Err(ServiceError::Validation(
                "the server's default workspace has no public link: every account joins it"
                    .to_string(),
            ));
        }
        Ok(self.repos.link_repo.link(server_id).await?)
    }

    /// The Owner's view of `link`, the workspace's link if it has one.
    async fn to_dto(
        &self,
        server_id: ServerId,
        link: Option<PublicJoinLink>,
    ) -> ServiceResult<PublicJoinLinkDto> {
        let now = self.clock.now();
        let link_repo = &self.repos.link_repo;
        let joins_last_day = link_repo
            .count_joins_since(server_id, now - Duration::hours(24))
            .await?;
        let joins_last_week = link_repo
            .count_joins_since(server_id, now - Duration::days(7))
            .await?;
        let open_requests = link_repo.count_open_for_server(server_id, now).await?;
        let token = link.as_ref().and_then(|l| {
            let token = self.link_key.as_ref()?.open(server_id, &l.sealed_token);
            if token.is_none() {
                warn!(server_id = %server_id, "the public link's token does not open under the link key");
            }
            token
        });
        let url = token
            .as_ref()
            .map(|t| format!("{}/join/{t}", self.link_base));
        let link = link.as_ref();
        Ok(PublicJoinLinkDto {
            server_id,
            enabled: link.is_some_and(|l| l.enabled),
            mode: link.map_or(PublicLinkMode::Direct, |l| l.mode),
            role: link.map_or(ServerMemberRole::Member, |l| l.role),
            allow_remote: link.is_none_or(|l| l.allow_remote),
            enabled_at: link.and_then(|l| l.enabled_at),
            issued_at: link.map(|l| l.issued_at),
            issued_by: link.and_then(|l| l.issued_by),
            token,
            url,
            joins_last_day,
            joins_last_week,
            open_requests,
        })
    }

    /// A link for `server_id` with a fresh token issued by `actor` now, the other settings from
    /// `base` or the defaults (off, direct, Member, accounts from other servers allowed).
    fn issue(
        &self,
        server_id: ServerId,
        actor: UserId,
        base: Option<&PublicJoinLink>,
    ) -> ServiceResult<PublicJoinLink> {
        let token = rim_auth::session_token::generate_public_link_token();
        let sealed_token = self
            .link_key
            .as_ref()
            .ok_or(ServiceError::Unavailable)?
            .seal(server_id, &token)
            .map_err(ServiceError::internal)?;
        Ok(PublicJoinLink {
            server_id,
            token_hash: rim_auth::session_token::hash_token(&token),
            sealed_token,
            enabled: base.is_some_and(|l| l.enabled),
            mode: base.map_or(PublicLinkMode::Direct, |l| l.mode),
            role: base.map_or(ServerMemberRole::Member, |l| l.role),
            allow_remote: base.is_none_or(|l| l.allow_remote),
            enabled_at: base.and_then(|l| l.enabled_at),
            issued_at: self.clock.now(),
            issued_by: Some(actor),
        })
    }

    /// The switched-on link `token` names and its workspace, looked up by the token's hash
    /// whatever the token is. A miss is logged and counted against `remote_ip`.
    ///
    /// # Errors
    /// `NotFound` for every invalid link.
    async fn valid_link(
        &self,
        repos: &PublicJoinLinkRepos,
        token: &str,
        remote_ip: &str,
    ) -> ServiceResult<(PublicJoinLink, Server)> {
        let token_hash = rim_auth::session_token::hash_token(token);
        let link = repos.link_repo.link_by_token_hash(&token_hash).await?;
        let found = match link.filter(|l| l.enabled) {
            Some(link) => repos
                .server_repo
                .get_by_id(link.server_id)
                .await?
                .map(|server| (link, server)),
            None => None,
        };
        found.ok_or_else(|| {
            self.misses.record(remote_ip, self.clock.now());
            info!(remote_ip, "public join link lookup missed");
            ServiceError::NotFound
        })
    }

    /// The workspace's name for a join request answer; empty when the workspace is gone.
    async fn workspace_name(&self, server_id: ServerId) -> ServiceResult<String> {
        Ok(self
            .repos
            .server_repo
            .get_by_id(server_id)
            .await?
            .map(|s| s.name)
            .unwrap_or_default())
    }

    /// `PermissionDenied` unless `actor` is the workspace's Owner or a Moderator.
    async fn ensure_moderator(&self, actor: Actor, server_id: ServerId) -> ServiceResult<()> {
        let member = self
            .repos
            .server_member_repo
            .get_member(server_id, actor.user_id)
            .await?
            .ok_or(ServiceError::PermissionDenied)?;
        if !member.member_role.is_moderator() {
            return Err(ServiceError::PermissionDenied);
        }
        Ok(())
    }

    /// The workspace's open request `request_id`, read with `repos`.
    ///
    /// # Errors
    /// `NotFound` for a request of another workspace, `Conflict` when it is no longer open.
    async fn open_request_of(
        repos: &PublicJoinLinkRepos,
        server_id: ServerId,
        request_id: JoinRequestId,
        now: DateTime<Utc>,
    ) -> ServiceResult<JoinRequest> {
        let request = repos
            .link_repo
            .request(request_id)
            .await?
            .filter(|r| r.server_id == server_id)
            .ok_or(ServiceError::NotFound)?;
        if !request.is_open_at(now) {
            return Err(no_longer_open());
        }
        Ok(request)
    }

    /// Add `user_id` to the workspace with `role` through the link, with `repos`, and record
    /// the join for the link's counts.
    ///
    /// # Errors
    /// `Conflict` for a member, `Forbidden` while banned, `Validation` when the workspace is
    /// full.
    async fn admit(
        &self,
        repos: &PublicJoinLinkRepos,
        server_id: ServerId,
        user_id: UserId,
        role: ServerMemberRole,
        now: DateTime<Utc>,
    ) -> ServiceResult<()> {
        ensure_not_member(repos, server_id, user_id).await?;
        ensure_not_banned(repos.moderation_repo.as_ref(), server_id, user_id).await?;
        add_workspace_member(
            repos.server_member_repo.as_ref(),
            repos.conversation_repo.as_ref(),
            repos.conversation_member_repo.as_ref(),
            server_id,
            user_id,
            role,
            self.limits.members_per_workspace,
        )
        .await?;
        repos.link_repo.record_join(server_id, user_id, now).await?;
        Ok(())
    }

    /// Announce and audit a join through the link, after the transaction committed.
    async fn announce_joined(
        &self,
        server_id: ServerId,
        user_id: UserId,
        role: ServerMemberRole,
        join_request_id: Option<JoinRequestId>,
    ) {
        match self.repos.user_repo.get_by_id(user_id).await {
            Ok(Some(user)) => self.observers.emit(WorkspaceEvent::MemberJoined {
                workspace_id: server_id,
                user_id,
                username: user.username,
                source: Some(PUBLIC_LINK_SOURCE.to_string()),
            }),
            Ok(None) => {}
            Err(e) => warn!(error = %e, user_id = %user_id, "member join not announced"),
        }
        self.audit(
            user_id,
            server_id,
            audit_ids::MEMBER_JOINED,
            serde_json::json!({
                "user_id": user_id.to_string(),
                "source": PUBLIC_LINK_SOURCE,
                "role": role.as_str(),
                "join_request_id": join_request_id.map(|id| id.to_string()),
            }),
        )
        .await;
        info!(user_id = %user_id, server_id = %server_id, "user joined a workspace through its public link");
    }

    /// File a join request for `actor` with `repos`, after the checks a join would run.
    async fn file_request(
        &self,
        repos: &PublicJoinLinkRepos,
        server_id: ServerId,
        actor: UserId,
        message: Option<String>,
        now: DateTime<Utc>,
    ) -> ServiceResult<JoinRequest> {
        ensure_not_member(repos, server_id, actor).await?;
        ensure_not_banned(repos.moderation_repo.as_ref(), server_id, actor).await?;
        if self.limits.members_per_workspace.is_some() {
            let members = repos.server_member_repo.count_members(server_id).await?;
            ensure_below(
                self.limits.members_per_workspace,
                members,
                "members per workspace",
            )?;
        }
        if repos
            .link_repo
            .open_request(server_id, actor, now)
            .await?
            .is_some()
        {
            return Err(ServiceError::Conflict(
                "you already asked to join this workspace".to_string(),
            ));
        }
        if repos.link_repo.count_open_for_user(actor, now).await? >= MAX_OPEN_JOIN_REQUESTS_PER_USER
        {
            return Err(ServiceError::Validation(format!(
                "you can have at most {MAX_OPEN_JOIN_REQUESTS_PER_USER} open join requests; \
                 withdraw one first"
            )));
        }
        if let Some(declined_at) = repos.link_repo.last_declined_at(server_id, actor).await? {
            let again_at = declined_at + Duration::hours(JOIN_REQUEST_RETRY_HOURS);
            if again_at > now {
                let hours = ((again_at - now).num_minutes() + 59) / 60;
                let unit = if hours == 1 { "hour" } else { "hours" };
                return Err(ServiceError::Validation(format!(
                    "your request was declined; you can ask to join this workspace again in \
                     {hours} {unit}"
                )));
            }
        }
        Ok(repos
            .link_repo
            .create_request(NewJoinRequest {
                server_id,
                user_id: actor,
                message,
                created_at: now,
                expires_at: now + Duration::days(JOIN_REQUEST_DAYS),
            })
            .await?)
    }
}

/// `Conflict` when `user_id` is already a member of the workspace.
async fn ensure_not_member(
    repos: &PublicJoinLinkRepos,
    server_id: ServerId,
    user_id: UserId,
) -> ServiceResult<()> {
    if repos
        .server_member_repo
        .is_member(server_id, user_id)
        .await?
    {
        return Err(ServiceError::Conflict(
            "already a member of this workspace".to_string(),
        ));
    }
    Ok(())
}

fn no_longer_open() -> ServiceError {
    ServiceError::Conflict("this join request is no longer open".to_string())
}

#[async_trait]
impl PublicJoinLinkService for DefaultPublicJoinLinkService {
    async fn public_join_link(
        &self,
        actor: Actor,
        server_id: ServerId,
    ) -> ServiceResult<PublicJoinLinkDto> {
        let link = self.owner_link(actor, server_id).await?;
        self.to_dto(server_id, link).await
    }

    async fn update_public_join_link(
        &self,
        actor: Actor,
        server_id: ServerId,
        patch: PublicJoinLinkPatch,
    ) -> ServiceResult<PublicJoinLinkDto> {
        if patch
            .role
            .is_some_and(|r| !matches!(r, ServerMemberRole::Member | ServerMemberRole::Guest))
        {
            return Err(ServiceError::Validation(
                "a public link makes people MEMBER or GUEST".to_string(),
            ));
        }
        let existing = self.owner_link(actor, server_id).await?;
        let now = self.clock.now();
        let mut link = match &existing {
            Some(link) => link.clone(),
            None => self.issue(server_id, actor.user_id, None)?,
        };
        let was_enabled = existing.as_ref().is_some_and(|l| l.enabled);
        if let Some(enabled) = patch.enabled {
            link.enabled = enabled;
            if enabled && !was_enabled {
                link.enabled_at = Some(now);
            }
        }
        let before = (link.mode, link.role, link.allow_remote);
        link.mode = patch.mode.unwrap_or(link.mode);
        link.role = patch.role.unwrap_or(link.role);
        link.allow_remote = patch.allow_remote.unwrap_or(link.allow_remote);
        let settings_changed =
            existing.is_some() && before != (link.mode, link.role, link.allow_remote);
        let saved = self.repos.link_repo.save_link(link).await?;

        if saved.enabled != was_enabled {
            let id = if saved.enabled {
                audit_ids::ENABLED
            } else {
                audit_ids::DISABLED
            };
            self.audit(
                actor.user_id,
                server_id,
                id,
                serde_json::json!({
                    "mode": saved.mode.as_str(),
                    "role": saved.role.as_str(),
                    "allow_remote": saved.allow_remote,
                }),
            )
            .await;
        }
        if settings_changed {
            self.audit(
                actor.user_id,
                server_id,
                audit_ids::SETTINGS_CHANGED,
                serde_json::json!({
                    "mode": saved.mode.as_str(),
                    "role": saved.role.as_str(),
                    "allow_remote": saved.allow_remote,
                }),
            )
            .await;
        }
        self.to_dto(server_id, Some(saved)).await
    }

    async fn reissue_public_join_link(
        &self,
        actor: Actor,
        server_id: ServerId,
    ) -> ServiceResult<PublicJoinLinkDto> {
        let existing = self.owner_link(actor, server_id).await?;
        let link = self.issue(server_id, actor.user_id, existing.as_ref())?;
        let saved = self.repos.link_repo.save_link(link).await?;
        self.audit(
            actor.user_id,
            server_id,
            audit_ids::REISSUED,
            serde_json::json!({ "enabled": saved.enabled }),
        )
        .await;
        self.to_dto(server_id, Some(saved)).await
    }

    async fn preview(
        &self,
        actor: Option<Actor>,
        token: String,
        remote_ip: String,
    ) -> ServiceResult<PublicJoinLinkPreviewDto> {
        let now = self.clock.now();
        self.misses.check(&remote_ip, now)?;
        let (link, server) = self.valid_link(&self.repos, &token, &remote_ip).await?;
        let mut join_request_id = None;
        let viewer = match actor {
            None => PublicLinkViewer::SignedOut,
            Some(actor) => {
                let repos = &self.repos;
                if repos
                    .server_member_repo
                    .is_member(server.id, actor.user_id)
                    .await?
                {
                    PublicLinkViewer::Member
                } else if repos
                    .moderation_repo
                    .get_active_ban(server.id, actor.user_id, now)
                    .await?
                    .is_some()
                {
                    PublicLinkViewer::Banned
                } else if let Some(request) = repos
                    .link_repo
                    .open_request(server.id, actor.user_id, now)
                    .await?
                {
                    join_request_id = Some(request.id);
                    PublicLinkViewer::RequestOpen
                } else {
                    PublicLinkViewer::NotMember
                }
            }
        };
        Ok(PublicJoinLinkPreviewDto {
            server_id: server.id,
            name: server.name,
            mode: link.mode,
            viewer,
            join_request_id,
        })
    }

    async fn join(
        &self,
        actor: Actor,
        token: String,
        message: Option<String>,
        remote_ip: String,
    ) -> ServiceResult<JoinByPublicLinkResultDto> {
        let now = self.clock.now();
        self.misses.check(&remote_ip, now)?;
        let (scope, repos) = self.transaction().await?;
        let (link, server) = self.valid_link(&repos, &token, &remote_ip).await?;
        match link.mode {
            PublicLinkMode::Direct => {
                self.admit(&repos, server.id, actor.user_id, link.role, now)
                    .await?;
                Self::commit(scope).await?;
                self.announce_joined(server.id, actor.user_id, link.role, None)
                    .await;
                Ok(JoinByPublicLinkResultDto {
                    outcome: JoinByPublicLinkOutcome::Joined,
                    server_id: server.id,
                    join_request: None,
                })
            }
            PublicLinkMode::Requests => {
                let message = normalize_message(message)?;
                let request = self
                    .file_request(&repos, server.id, actor.user_id, message, now)
                    .await?;
                Self::commit(scope).await?;
                match request_audience(
                    self.repos.server_member_repo.as_ref(),
                    server.id,
                    actor.user_id,
                )
                .await
                {
                    Ok(member_user_ids) => {
                        self.observers.emit(WorkspaceEvent::JoinRequestCreated {
                            workspace_id: server.id,
                            request_id: request.id,
                            user_id: actor.user_id,
                            member_user_ids,
                        })
                    }
                    Err(e) => {
                        warn!(error = %e, request_id = %request.id, "join request not announced")
                    }
                }
                info!(user_id = %actor.user_id, server_id = %server.id, "join request filed");
                Ok(JoinByPublicLinkResultDto {
                    outcome: JoinByPublicLinkOutcome::Requested,
                    server_id: server.id,
                    join_request: Some(JoinRequestDto::from_request(request, server.name, now)),
                })
            }
        }
    }

    async fn my_join_requests(&self, actor: Actor) -> ServiceResult<Vec<JoinRequestDto>> {
        let now = self.clock.now();
        let mut answer = Vec::new();
        for request in self
            .repos
            .link_repo
            .list_open_for_user(actor.user_id, now)
            .await?
        {
            let name = self.workspace_name(request.server_id).await?;
            answer.push(JoinRequestDto::from_request(request, name, now));
        }
        Ok(answer)
    }

    async fn withdraw_join_request(
        &self,
        actor: Actor,
        request_id: JoinRequestId,
    ) -> ServiceResult<JoinRequestDto> {
        let now = self.clock.now();
        let request = self
            .repos
            .link_repo
            .request(request_id)
            .await?
            .filter(|r| r.user_id == actor.user_id)
            .ok_or(ServiceError::NotFound)?;
        let closed = self
            .repos
            .link_repo
            .close_request(
                request.id,
                JoinRequestStatus::Withdrawn,
                Some(actor.user_id),
                now,
            )
            .await?
            .ok_or_else(no_longer_open)?;
        announce_decided(
            self.repos.server_member_repo.as_ref(),
            &self.observers,
            &closed,
            JoinRequestStatus::Withdrawn,
        )
        .await;
        let name = self.workspace_name(closed.server_id).await?;
        Ok(JoinRequestDto::from_request(closed, name, now))
    }

    async fn list_join_requests(
        &self,
        query: ListJoinRequestsQuery,
    ) -> ServiceResult<Page<JoinRequestDto, JoinRequestPageCursor>> {
        self.ensure_moderator(query.actor, query.server_id).await?;
        let now = self.clock.now();
        let page = self
            .repos
            .link_repo
            .list_open_requests(ListJoinRequestsParams {
                server_id: query.server_id,
                now,
                limit: query.limit,
                dir: query.dir,
                cursor: query.cursor.map(Into::into),
            })
            .await?;
        let name = self.workspace_name(query.server_id).await?;
        Ok(Page::new(
            page.items
                .into_iter()
                .map(|r| JoinRequestDto::from_request(r, name.clone(), now))
                .collect(),
            page.next.map(Into::into),
            page.prev.map(Into::into),
        ))
    }

    async fn accept_join_request(
        &self,
        actor: Actor,
        server_id: ServerId,
        request_id: JoinRequestId,
    ) -> ServiceResult<JoinRequestDto> {
        self.ensure_moderator(actor, server_id).await?;
        let now = self.clock.now();
        let (scope, repos) = self.transaction().await?;
        let request = Self::open_request_of(&repos, server_id, request_id, now).await?;
        // A person who joined another way since asking has nothing left to accept: the
        // request is closed as accepted, without a second join.
        if repos
            .server_member_repo
            .is_member(server_id, request.user_id)
            .await?
        {
            let closed = repos
                .link_repo
                .close_request(
                    request.id,
                    JoinRequestStatus::Accepted,
                    Some(actor.user_id),
                    now,
                )
                .await?
                .ok_or_else(no_longer_open)?;
            Self::commit(scope).await?;
            announce_decided(
                self.repos.server_member_repo.as_ref(),
                &self.observers,
                &closed,
                JoinRequestStatus::Accepted,
            )
            .await;
            let name = self.workspace_name(server_id).await?;
            return Ok(JoinRequestDto::from_request(closed, name, now));
        }
        let role = repos
            .link_repo
            .link(server_id)
            .await?
            .map_or(ServerMemberRole::Member, |l| l.role);
        let closed = repos
            .link_repo
            .close_request(
                request.id,
                JoinRequestStatus::Accepted,
                Some(actor.user_id),
                now,
            )
            .await?
            .ok_or_else(no_longer_open)?;
        self.admit(&repos, server_id, request.user_id, role, now)
            .await?;
        Self::commit(scope).await?;

        self.audit(
            actor.user_id,
            server_id,
            audit_ids::REQUEST_ACCEPTED,
            serde_json::json!({
                "join_request_id": request.id.to_string(),
                "user_id": request.user_id.to_string(),
                "role": role.as_str(),
            }),
        )
        .await;
        self.announce_joined(server_id, request.user_id, role, Some(request.id))
            .await;
        announce_decided(
            self.repos.server_member_repo.as_ref(),
            &self.observers,
            &closed,
            JoinRequestStatus::Accepted,
        )
        .await;
        let name = self.workspace_name(server_id).await?;
        Ok(JoinRequestDto::from_request(closed, name, now))
    }

    async fn decline_join_request(
        &self,
        actor: Actor,
        server_id: ServerId,
        request_id: JoinRequestId,
    ) -> ServiceResult<JoinRequestDto> {
        self.ensure_moderator(actor, server_id).await?;
        let now = self.clock.now();
        let request = Self::open_request_of(&self.repos, server_id, request_id, now).await?;
        let closed = self
            .repos
            .link_repo
            .close_request(
                request.id,
                JoinRequestStatus::Declined,
                Some(actor.user_id),
                now,
            )
            .await?
            .ok_or_else(no_longer_open)?;
        self.audit(
            actor.user_id,
            server_id,
            audit_ids::REQUEST_DECLINED,
            serde_json::json!({
                "join_request_id": closed.id.to_string(),
                "user_id": closed.user_id.to_string(),
            }),
        )
        .await;
        announce_decided(
            self.repos.server_member_repo.as_ref(),
            &self.observers,
            &closed,
            JoinRequestStatus::Declined,
        )
        .await;
        let name = self.workspace_name(server_id).await?;
        Ok(JoinRequestDto::from_request(closed, name, now))
    }
}
