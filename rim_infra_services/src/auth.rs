use async_trait::async_trait;
use chrono::{Duration, Utc};
use regex::Regex;
use sea_orm::DatabaseConnection;
use std::sync::OnceLock;
use tracing::{info, instrument, warn};

use rim_auth::session_token::{generate_token, hash_token};
use rim_core::auth::model::NewInvite;
use rim_core::auth::repository::ListInvitesParams;
use rim_core::repository::pagination::Page;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;
use rim_infra_db::begin_transaction;
use rim_services::auth::command::{
    CreateInviteCmd, JoinServerByInviteCmd, ListInvitesQuery, LogoutCmd, LogoutOtherSessionsCmd,
    PreviewInviteTokenQuery, RevokeInviteCmd,
};
use rim_services::auth::dto::{
    CreateInviteResultDto, InviteDto, InvitePageCursor, InviteTokenPreviewDto, InviteTokenType,
    JoinServerByInviteResultDto, WorkspaceInvitePreviewDto,
};
use rim_services::auth::service::AuthService;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::limits::{InstanceLimits, ensure_below, limit_reached};
use rim_services::session_revocation::SessionRevocations;
use rim_services::{ServiceError, ServiceResult};

use rim_services::events::{EventObserver, Observers, WorkspaceEvent};

use crate::AuthRepos;
use crate::account_cap::AccountCap;
use crate::email::{EmailSender, templates};
use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::repos::AppRepos;
use crate::url_encode::percent_encode_path_segment;
use crate::util::{add_workspace_member, ensure_not_banned, ensure_server_owner};

/// Fixed "IP" slot for the invite email limiter, whose buckets are keyed by user id alone.
const INVITE_EMAIL_SCOPE: &str = "invite_email";

/// The window of the invite email limiter: one day.
const INVITE_EMAIL_WINDOW_SECS: u64 = 86_400;

#[derive(Clone)]
pub struct DefaultAuthService {
    repos: AuthRepos,
    db: Option<DatabaseConnection>,
    observers: Observers,
    preview_rate_limiter: RegistrationRateLimiter,
    email_sender: std::sync::Arc<dyn EmailSender>,
    /// e.g. `https://rim.example.com`. Used to build the `/join/{token}` invite link. If unset,
    /// invite emails fall back to the bare token (dev-only).
    public_base_url: Option<String>,
    /// Caps on invites and on workspace members.
    limits: InstanceLimits,
    /// Invite emails one user sends per day, at the invites-per-day cap; `None` when that cap is
    /// off. Counted apart from the invites, which go when their workspace is deleted.
    invite_email_rate_limiter: Option<RegistrationRateLimiter>,
    /// Told of every session a logout ends, so the session's live connections close.
    session_revocations: std::sync::Arc<dyn SessionRevocations>,
    /// The cap on local accounts the key signup checks.
    account_cap: AccountCap,
}

impl DefaultAuthService {
    /// Construct a new auth service with the default preview rate limiter.
    ///
    /// # Examples
    /// ```ignore
    /// let svc = DefaultAuthService::new(repos, Some(db), observers, email_sender, None);
    /// ```
    pub fn new(
        repos: AuthRepos,
        db: Option<DatabaseConnection>,
        observers: Observers,
        email_sender: std::sync::Arc<dyn EmailSender>,
        public_base_url: Option<String>,
    ) -> Self {
        Self {
            repos,
            db,
            observers,
            preview_rate_limiter: RegistrationRateLimiter::preview_default(),
            email_sender,
            public_base_url,
            limits: InstanceLimits::default(),
            invite_email_rate_limiter: None,
            session_revocations: crate::session_revocation::unshared(),
            account_cap: AccountCap::default(),
        }
    }

    /// Build the invite link as a path segment: `{public_base_url}/join/{token}` — never a
    /// `?token=` query string, which would leak the token via `Referer`. Falls back to the bare
    /// token (with a warning) if `public_base_url` is unset.
    fn build_invite_link(&self, raw_token: &str) -> String {
        let encoded = percent_encode_path_segment(raw_token);
        match &self.public_base_url {
            Some(base) => format!("{}/join/{}", base.trim_end_matches('/'), encoded),
            None => {
                warn!(
                    "RIM_PUBLIC_BASE_URL is not configured; invite email will contain the bare \
                     token instead of a clickable link"
                );
                raw_token.to_string()
            }
        }
    }

    /// Applies the instance's caps: invites, workspace members and, with a cap of its own
    /// that `with_account_cap` can replace by a shared one, local accounts.
    pub fn with_limits(mut self, limits: InstanceLimits) -> Self {
        self.limits = limits;
        self.account_cap = AccountCap::new(limits.accounts);
        self.invite_email_rate_limiter = limits
            .invites_per_user_per_day
            .map(|cap| RegistrationRateLimiter::new(INVITE_EMAIL_WINDOW_SECS, cap));
        self
    }

    /// `Validation` when an invite of `max_uses` uses would pass the instance's uses cap, or
    /// when `user_id` already created the instance's daily number of invites.
    async fn ensure_invite_limits(&self, user_id: UserId, max_uses: i32) -> ServiceResult<()> {
        if let Some(cap) = self.limits.invite_max_uses
            && i64::from(max_uses) > i64::from(cap)
        {
            return Err(limit_reached(cap, "uses per invite"));
        }
        if self.limits.invites_per_user_per_day.is_some() {
            let today = self
                .repos
                .invite_repo
                .count_created_since(user_id, Utc::now() - Duration::days(1))
                .await?;
            ensure_below(
                self.limits.invites_per_user_per_day,
                today,
                "invites per user per day",
            )?;
        }
        Ok(())
    }

    /// Count one invite email from `user_id` against the instance's invites-per-day cap.
    /// Returns when it was counted, so a failed invite can give it back; `None` when there is no
    /// cap.
    ///
    /// # Errors
    /// `Validation` naming the cap when `user_id` sent that many invite emails in the last 24
    /// hours.
    fn reserve_invite_email(
        &self,
        user_id: UserId,
    ) -> ServiceResult<Option<chrono::DateTime<Utc>>> {
        let (Some(limiter), Some(cap)) = (
            &self.invite_email_rate_limiter,
            self.limits.invites_per_user_per_day,
        ) else {
            return Ok(None);
        };
        let now = Utc::now();
        match limiter.check_and_record(INVITE_EMAIL_SCOPE, &user_id.to_string(), now) {
            RateLimitDecision::Allowed => Ok(Some(now)),
            RateLimitDecision::Limited { .. } => {
                Err(limit_reached(cap, "invite emails per user per day"))
            }
        }
    }

    /// Answers `registrationFull` from `cap`, built from the same limits and shared with the key
    /// signup, so a signup refreshes the answer.
    pub(crate) fn with_account_cap(mut self, cap: AccountCap) -> Self {
        self.account_cap = cap;
        self
    }

    /// Report the sessions a logout ends to `revocations`.
    pub(crate) fn with_session_revocations(
        mut self,
        revocations: std::sync::Arc<dyn SessionRevocations>,
    ) -> Self {
        self.session_revocations = revocations;
        self
    }

    /// Use `limiter` for the invite token preview.
    #[cfg(test)]
    pub(crate) fn with_preview_rate_limiter(mut self, limiter: RegistrationRateLimiter) -> Self {
        self.preview_rate_limiter = limiter;
        self
    }

    /// Validate and normalise an email address.
    ///
    /// Trims outer whitespace, lowercases, checks a pragmatic `local@domain.tld`
    /// regex, and enforces a 1–254 character length. Returns the normalised
    /// (trimmed + lowercased) email on success so callers can use the returned
    /// value directly for both the uniqueness check and the insert.
    ///
    /// # Errors
    /// - `ServiceError::Validation("invalid email")` if the format is wrong or
    ///   the length is outside `1..=254` after trimming.
    ///
    /// # Examples
    /// ```ignore
    /// let email = DefaultAuthService::validate_email("  Foo@Bar.COM  ")?;
    /// assert_eq!(email, "foo@bar.com");
    /// ```
    pub(crate) fn validate_email(email: &str) -> ServiceResult<String> {
        static EMAIL_RE: OnceLock<Regex> = OnceLock::new();
        let re = EMAIL_RE.get_or_init(|| {
            Regex::new(r"^[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}$")
                .expect("email regex is valid")
        });

        let trimmed = email.trim();
        let len = trimmed.len();
        if len == 0 || len > 254 {
            return Err(ServiceError::Validation("invalid email".to_string()));
        }
        if !re.is_match(trimmed) {
            return Err(ServiceError::Validation("invalid email".to_string()));
        }
        Ok(trimmed.to_lowercase())
    }

    fn to_invite_dto(inv: rim_core::auth::model::Invite) -> InviteDto {
        InviteDto {
            id: inv.id,
            server_id: inv.server_id,
            created_by_user_id: inv.created_by_user_id,
            created_at: inv.created_at,
            expires_at: inv.expires_at,
            max_uses: inv.max_uses,
            use_count: inv.use_count,
            revoked_at: inv.revoked_at,
            role: inv.role,
        }
    }
}

#[async_trait]
impl AuthService for DefaultAuthService {
    #[instrument(name = "auth.logout", skip_all, fields(actor_id = %cmd.actor.user_id, session_id = %cmd.session_id))]
    async fn logout(&self, cmd: LogoutCmd) -> ServiceResult<()> {
        let LogoutCmd { actor, session_id } = cmd;
        let sess = self
            .repos
            .session_repo
            .get_by_id(session_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if sess.user_id != actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        self.repos.session_repo.revoke_session(session_id).await?;
        self.session_revocations.revoke(&[session_id]);

        Ok(())
    }

    #[instrument(name = "auth.logout_other_sessions", skip_all, fields(actor_id = %cmd.actor.user_id, session_id = %cmd.session_id))]
    async fn logout_other_sessions(&self, cmd: LogoutOtherSessionsCmd) -> ServiceResult<u64> {
        let LogoutOtherSessionsCmd { actor, session_id } = cmd;
        let sess = self
            .repos
            .session_repo
            .get_by_id(session_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if sess.user_id != actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        // One statement, so the sessions end together.
        let ended_sessions = self
            .repos
            .session_repo
            .revoke_all_for_user_except(actor.user_id, session_id)
            .await?;
        self.session_revocations
            .revoke_user_except(actor.user_id, session_id);

        info!(
            target: "audit",
            event = "other_sessions_signed_out",
            user_id = %actor.user_id,
            kept_session_id = %session_id,
            ended_sessions,
            "other sessions signed out"
        );
        Ok(ended_sessions)
    }

    #[instrument(name = "auth.create_invite", skip_all, fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, max_uses))]
    async fn create_invite(&self, cmd: CreateInviteCmd) -> ServiceResult<CreateInviteResultDto> {
        let CreateInviteCmd {
            actor,
            server_id,
            expires_at,
            max_uses,
            recipient_email,
            role,
        } = cmd;
        self.ensure_invite_limits(actor.user_id, max_uses).await?;

        if max_uses <= 0 {
            return Err(ServiceError::Validation("max_uses must be > 0".to_string()));
        }
        // An invite grants `Member` or the restricted `Guest`, never a moderating role.
        if !matches!(role, ServerMemberRole::Member | ServerMemberRole::Guest) {
            return Err(ServiceError::Validation(
                "an invite can grant only the member or guest role".to_string(),
            ));
        }

        // Validate before persisting: a malformed address should surface
        // as a validation error, not a silently-dropped invite email.
        let recipient_email = recipient_email
            .map(|email| Self::validate_email(&email))
            .transpose()?;
        let expires_at = crate::instance_invite::expiry(expires_at, Utc::now())?;

        ensure_server_owner(
            self.repos.server_member_repo.as_ref(),
            server_id,
            actor.user_id,
        )
        .await?;
        let email_slot = if recipient_email.is_some() {
            self.reserve_invite_email(actor.user_id)?
        } else {
            None
        };

        let raw_token = generate_token("rim_inv_");
        let token_hash = hash_token(&raw_token);

        let created = self
            .repos
            .invite_repo
            .create_invite(NewInvite {
                server_id,
                created_by_user_id: actor.user_id,
                token_hash,
                expires_at: Some(expires_at),
                max_uses,
                role,
            })
            .await;
        let inv = match created {
            Ok(inv) => inv,
            Err(e) => {
                if let (Some(limiter), Some(at)) = (&self.invite_email_rate_limiter, email_slot) {
                    limiter.refund(INVITE_EMAIL_SCOPE, &actor.user_id.to_string(), at);
                }
                return Err(e.into());
            }
        };
        info!(
            target: "audit",
            event = "workspace_invite_created",
            invite_id = %inv.id,
            server_id = %inv.server_id,
            actor_id = %actor.user_id,
            "workspace invite created"
        );

        if let Some(recipient_email) = recipient_email {
            let link = self.build_invite_link(&raw_token);
            let message = templates::invite(&recipient_email, &link);
            if let Err(e) = self.email_sender.send(message).await {
                warn!(error = %e, "failed to send invite email");
            }
        }

        Ok(CreateInviteResultDto {
            invite: Self::to_invite_dto(inv),
            invite_token: raw_token,
        })
    }

    #[instrument(name = "auth.revoke_invite", skip_all, fields(actor_id = %cmd.actor.user_id, invite_id = %cmd.invite_id))]
    async fn revoke_invite(&self, cmd: RevokeInviteCmd) -> ServiceResult<InviteDto> {
        let RevokeInviteCmd { actor, invite_id } = cmd;
        let invite = self
            .repos
            .invite_repo
            .get_by_id(invite_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        // Outside the invite's workspace the invite does not exist; inside, only its owner
        // revokes.
        if !self
            .repos
            .server_member_repo
            .is_member(invite.server_id, actor.user_id)
            .await?
        {
            return Err(ServiceError::NotFound);
        }
        ensure_server_owner(
            self.repos.server_member_repo.as_ref(),
            invite.server_id,
            actor.user_id,
        )
        .await?;

        if invite.revoked_at.is_some() {
            return Ok(Self::to_invite_dto(invite));
        }
        self.repos.invite_repo.revoke(invite_id, Utc::now()).await?;
        let revoked = self
            .repos
            .invite_repo
            .get_by_id(invite_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        info!(
            target: "audit",
            event = "workspace_invite_revoked",
            invite_id = %revoked.id,
            server_id = %revoked.server_id,
            actor_id = %actor.user_id,
            "workspace invite revoked"
        );
        Ok(Self::to_invite_dto(revoked))
    }

    #[instrument(name = "auth.list_invites", skip_all, fields(actor_id = %query.actor.user_id, server_id = %query.server_id))]
    async fn list_invites(
        &self,
        query: ListInvitesQuery,
    ) -> ServiceResult<Page<InviteDto, InvitePageCursor>> {
        ensure_server_owner(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;
        let page = self
            .repos
            .invite_repo
            .list_for_server(ListInvitesParams {
                server_id: query.server_id,
                limit: query.limit,
                dir: query.dir,
                cursor: query.cursor.map(Into::into),
            })
            .await?;
        Ok(Page::new(
            page.items.into_iter().map(Self::to_invite_dto).collect(),
            page.next.map(Into::into),
            page.prev.map(Into::into),
        ))
    }

    #[instrument(name = "auth.join_server_by_invite", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn join_server_by_invite(
        &self,
        cmd: JoinServerByInviteCmd,
    ) -> ServiceResult<JoinServerByInviteResultDto> {
        let JoinServerByInviteCmd {
            actor,
            invite_token,
        } = cmd;

        let token_hash = hash_token(&invite_token);
        let inv = self
            .repos
            .invite_repo
            .get_by_token_hash(&token_hash)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if inv.revoked_at.is_some() {
            return Err(ServiceError::PermissionDenied);
        }
        if let Some(expires_at) = inv.expires_at
            && expires_at < Utc::now()
        {
            return Err(ServiceError::PermissionDenied);
        }

        // Check if user is already a member of the server.
        let already_member = self
            .repos
            .server_member_repo
            .is_member(inv.server_id, actor.user_id)
            .await?;
        if already_member {
            return Err(ServiceError::Conflict(
                "already a member of this server".to_string(),
            ));
        }
        // Banned users cannot re-join. Checked before the invite is consumed.
        ensure_not_banned(
            self.repos.moderation_repo.as_ref(),
            inv.server_id,
            actor.user_id,
        )
        .await?;

        // Run all writes inside a single transaction when DB is available.
        let scope = if let Some(db) = &self.db {
            Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            )
        } else {
            None
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_auth(),
            None => self.repos.clone(),
        };

        let consumed = repos.invite_repo.try_consume(inv.id).await?;
        if !consumed {
            return Err(ServiceError::PermissionDenied);
        }

        let _member = add_workspace_member(
            repos.server_member_repo.as_ref(),
            repos.conversation_repo.as_ref(),
            repos.conversation_member_repo.as_ref(),
            inv.server_id,
            actor.user_id,
            inv.role,
            self.limits.members_per_workspace,
        )
        .await?;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        // Emit MemberJoined event so other workspace members see the new member in real-time.
        if let Ok(Some(user)) = self
            .repos
            .user_repo
            .get_by_id(actor.user_id)
            .await
            .map_err(ServiceError::from)
        {
            self.observers.emit(WorkspaceEvent::MemberJoined {
                workspace_id: inv.server_id,
                user_id: actor.user_id,
                username: user.username.clone(),
                source: None,
            });
        }

        info!(user_id = %actor.user_id, server_id = %inv.server_id, "user joined server via invite");

        Ok(JoinServerByInviteResultDto {
            server_id: inv.server_id,
        })
    }

    #[instrument(name = "auth.preview_invite_token", skip_all, fields(remote_ip = %query.remote_ip))]
    async fn preview_invite_token(
        &self,
        query: PreviewInviteTokenQuery,
    ) -> ServiceResult<InviteTokenPreviewDto> {
        // Rate limit check first.
        let decision = self.preview_rate_limiter.check_and_record(
            &query.remote_ip,
            "preview_invite",
            Utc::now(),
        );
        if let rim_services::command::rate_limit::RateLimitDecision::Limited {
            retry_after_seconds,
        } = decision
        {
            tracing::warn!(
                target: "audit",
                event = "preview_invite_token_rate_limited",
                remote_ip = %query.remote_ip,
                "preview_invite_token rate limit exceeded"
            );
            crate::metrics::rate_limited("preview_invite");
            return Err(ServiceError::RateLimited {
                retry_after_seconds,
            });
        }

        let token_hash = hash_token(&query.token);

        // 1. Check instance_invite.
        if let Some(ii) = self
            .repos
            .instance_invite_repo
            .get_by_token_hash(&token_hash)
            .await?
        {
            if ii.revoked_at.is_some() {
                return Err(ServiceError::InvalidToken);
            }
            // Exhausted instance invites collapse to InvalidToken (consumed ≡ invalid).
            if ii.use_count >= ii.max_uses {
                return Err(ServiceError::InvalidToken);
            }
            if ii.expires_at < Utc::now() {
                return Err(ServiceError::ExpiredToken);
            }
            return Ok(InviteTokenPreviewDto {
                token_type: InviteTokenType::Instance,
                expires_at: Some(ii.expires_at),
                workspace: None,
            });
        }

        // 2. Check workspace invite.
        if let Some(inv) = self
            .repos
            .invite_repo
            .get_by_token_hash(&token_hash)
            .await?
        {
            if inv.revoked_at.is_some() {
                return Err(ServiceError::InvalidToken);
            }
            // Exhausted workspace invites are indistinguishable from invalid.
            if inv.use_count >= inv.max_uses {
                return Err(ServiceError::InvalidToken);
            }
            if let Some(expires_at) = inv.expires_at
                && expires_at < Utc::now()
            {
                return Err(ServiceError::ExpiredToken);
            }

            // Fetch workspace name and the inviter's and owner's display names.
            let server = self
                .repos
                .server_repo
                .get_by_id(inv.server_id)
                .await?
                .ok_or(ServiceError::NotFound)?;
            let inviter = self
                .repos
                .user_repo
                .get_by_id(inv.created_by_user_id)
                .await?
                .ok_or(ServiceError::NotFound)?;
            let inviter_display = inviter
                .display_name
                .unwrap_or_else(|| inviter.username.clone());
            let owner = match self
                .repos
                .server_member_repo
                .get_owner_id(inv.server_id)
                .await?
            {
                Some(owner_id) => self.repos.user_repo.get_by_id(owner_id).await?,
                None => None,
            };
            let owner_display = owner.map(|o| o.display_name.unwrap_or(o.username));

            return Ok(InviteTokenPreviewDto {
                token_type: InviteTokenType::Workspace,
                expires_at: inv.expires_at,
                workspace: Some(WorkspaceInvitePreviewDto {
                    server_id: inv.server_id,
                    server_name: server.name,
                    invited_by_display_name: inviter_display,
                    owner_display_name: owner_display,
                }),
            });
        }

        Err(ServiceError::InvalidToken)
    }

    async fn check_registration_capacity(&self) -> ServiceResult<()> {
        self.account_cap.check(self.repos.user_repo.as_ref()).await
    }

    async fn registration_full(&self) -> ServiceResult<bool> {
        self.account_cap
            .is_full(self.repos.user_repo.as_ref())
            .await
    }
}
