use async_trait::async_trait;
use chrono::{Duration, Utc};
use regex::Regex;
use sea_orm::DatabaseConnection;
use std::sync::OnceLock;
use tracing::{info, instrument, warn};

use rim_auth::session_token::{generate_token, hash_token};
use rim_core::auth::model::NewInvite;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;
use rim_infra_db::begin_transaction;
use rim_services::auth::command::{
    AcceptInviteCmd, CreateInviteCmd, JoinServerByInviteCmd, LoginCmd, LogoutCmd,
    LogoutOtherSessionsCmd, PreviewInviteTokenQuery, SignupOpenCmd, SignupWithInstanceInviteCmd,
};
use rim_services::auth::dto::{
    AcceptInviteResultDto, AuthSessionDto, CreateInviteResultDto, InviteDto, InviteTokenPreviewDto,
    InviteTokenType, JoinServerByInviteResultDto, LoginResultDto, SignupResultDto,
    WorkspaceInvitePreviewDto,
};
use rim_services::auth::service::AuthService;
use rim_services::auth::terms::accepted_terms;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::limits::{InstanceLimits, ensure_below, limit_reached};
use rim_services::session_revocation::SessionRevocations;
use rim_services::{ServiceError, ServiceResult};

use rim_services::events::{EventObserver, Observers, WorkspaceEvent};

use crate::AuthRepos;
use crate::default_workspace;
use crate::email::{EmailSender, templates};
use crate::login_sources::KnownLoginSources;
use crate::password_hashing::PasswordHashing;
use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::repos::AppRepos;
use crate::reserved_names::ReservedNames;
use crate::signup;
use crate::url_encode::percent_encode_path_segment;
use crate::util::{add_workspace_member, ensure_not_banned, ensure_server_owner};

/// Fixed "IP" slot for the per-account login limiter, whose buckets are keyed by username alone.
const LOGIN_ACCOUNT_SCOPE: &str = "account";

#[derive(Clone)]
pub struct DefaultAuthService {
    repos: AuthRepos,
    db: Option<DatabaseConnection>,
    session_ttl: Duration,
    observers: Observers,
    preview_rate_limiter: RegistrationRateLimiter,
    signup_rate_limiter: RegistrationRateLimiter,
    login_ip_rate_limiter: RegistrationRateLimiter,
    login_ip_account_rate_limiter: RegistrationRateLimiter,
    login_account_rate_limiter: RegistrationRateLimiter,
    /// Sources that recently signed in to an account; they skip the per-username backstop.
    login_known_sources: KnownLoginSources,
    email_sender: std::sync::Arc<dyn EmailSender>,
    /// e.g. `https://rim.example.com`. Used to build the `/join/{token}` invite link. If unset,
    /// invite emails fall back to the bare token (dev-only).
    public_base_url: Option<String>,
    /// Caps on invites and on workspace members.
    limits: InstanceLimits,
    /// Usernames and display names signups may not take.
    reserved_names: ReservedNames,
    /// Runs the Argon2 work of signups and logins.
    password_hashing: PasswordHashing,
    /// Told of every session a logout ends, so the session's live connections close.
    session_revocations: std::sync::Arc<dyn SessionRevocations>,
    /// The terms of service URL every signup must accept; `None` when there are none.
    terms_url: Option<String>,
    /// Refuse every password signup.
    signup_requires_keys: bool,
    /// The workspace every new account joins as a Member; `None` when there is none.
    default_workspace: Option<rim_core::server::model::ServerId>,
}

impl DefaultAuthService {
    /// Construct a new auth service with default rate limiters.
    ///
    /// # Examples
    /// ```ignore
    /// let svc = DefaultAuthService::new(
    ///     repos, Some(db), Duration::days(30), observers, email_sender, None,
    /// );
    /// ```
    pub fn new(
        repos: AuthRepos,
        db: Option<DatabaseConnection>,
        session_ttl: Duration,
        observers: Observers,
        email_sender: std::sync::Arc<dyn EmailSender>,
        public_base_url: Option<String>,
    ) -> Self {
        Self {
            repos,
            db,
            session_ttl,
            observers,
            preview_rate_limiter: RegistrationRateLimiter::preview_default(),
            signup_rate_limiter: RegistrationRateLimiter::signup_default(),
            login_ip_rate_limiter: RegistrationRateLimiter::login_ip_default(),
            login_ip_account_rate_limiter: RegistrationRateLimiter::login_ip_account_default(),
            login_account_rate_limiter: RegistrationRateLimiter::login_account_default(),
            login_known_sources: KnownLoginSources::login_default(),
            email_sender,
            public_base_url,
            limits: InstanceLimits::default(),
            reserved_names: ReservedNames::default(),
            password_hashing: PasswordHashing::default(),
            session_revocations: crate::session_revocation::unshared(),
            terms_url: None,
            signup_requires_keys: false,
            default_workspace: None,
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

    /// Replace the login rate limiters (per-IP attempts, per-(IP, username) failures,
    /// per-username failures from any IP).
    #[cfg(test)]
    pub(crate) fn with_login_rate_limiters(
        mut self,
        ip_limiter: RegistrationRateLimiter,
        ip_account_limiter: RegistrationRateLimiter,
        account_limiter: RegistrationRateLimiter,
    ) -> Self {
        self.login_ip_rate_limiter = ip_limiter;
        self.login_ip_account_rate_limiter = ip_account_limiter;
        self.login_account_rate_limiter = account_limiter;
        self
    }

    /// Use `limiter` for open-mode signups, so other signup paths (key-only signup) can share
    /// one per-IP budget with the password signup.
    pub(crate) fn with_signup_rate_limiter(mut self, limiter: RegistrationRateLimiter) -> Self {
        self.signup_rate_limiter = limiter;
        self
    }

    pub fn with_limits(mut self, limits: InstanceLimits) -> Self {
        self.limits = limits;
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

    pub(crate) fn with_reserved_names(mut self, reserved_names: ReservedNames) -> Self {
        self.reserved_names = reserved_names;
        self
    }

    /// Requires every signup to accept the terms at `terms_url`.
    pub fn with_terms_url(mut self, terms_url: Option<String>) -> Self {
        self.terms_url = terms_url;
        self
    }

    /// Refuse every password signup when `required`, so new accounts sign up with a key.
    pub fn with_signup_requires_keys(mut self, required: bool) -> Self {
        self.signup_requires_keys = required;
        self
    }

    /// Makes every new account a Member of `workspace`.
    pub fn with_default_workspace(
        mut self,
        workspace: Option<rim_core::server::model::ServerId>,
    ) -> Self {
        self.default_workspace = workspace;
        self
    }

    /// `Forbidden` ([`signup::KEYS_REQUIRED_REFUSAL`]) when password signups are refused.
    fn ensure_password_signup_allowed(&self, path: &str) -> ServiceResult<()> {
        if !self.signup_requires_keys {
            return Ok(());
        }
        info!(
            target: "audit",
            event = "password_signup_refused",
            path,
            "password signup refused: this server takes key signups only"
        );
        Err(ServiceError::Forbidden(
            signup::KEYS_REQUIRED_REFUSAL.to_string(),
        ))
    }

    /// Run hashing and verification through `hashing`, shared with other services.
    pub(crate) fn with_password_hashing(mut self, hashing: PasswordHashing) -> Self {
        self.password_hashing = hashing;
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

    /// Share the store of recent successful login sources, so other services (password
    /// reset) can forget an account's sources.
    pub(crate) fn with_known_login_sources(mut self, sources: KnownLoginSources) -> Self {
        self.login_known_sources = sources;
        self
    }

    /// Construct with explicit preview and signup rate limiters.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_rate_limiters(
        repos: AuthRepos,
        db: Option<DatabaseConnection>,
        session_ttl: Duration,
        observers: Observers,
        preview_rate_limiter: RegistrationRateLimiter,
        signup_rate_limiter: RegistrationRateLimiter,
        email_sender: std::sync::Arc<dyn EmailSender>,
        public_base_url: Option<String>,
    ) -> Self {
        Self {
            repos,
            db,
            session_ttl,
            observers,
            preview_rate_limiter,
            signup_rate_limiter,
            login_ip_rate_limiter: RegistrationRateLimiter::login_ip_default(),
            login_ip_account_rate_limiter: RegistrationRateLimiter::login_ip_account_default(),
            login_account_rate_limiter: RegistrationRateLimiter::login_account_default(),
            login_known_sources: KnownLoginSources::login_default(),
            email_sender,
            public_base_url,
            limits: InstanceLimits::default(),
            reserved_names: ReservedNames::default(),
            password_hashing: PasswordHashing::default(),
            session_revocations: crate::session_revocation::unshared(),
            terms_url: None,
            signup_requires_keys: false,
            default_workspace: None,
        }
    }

    fn validate_password(password: &str) -> ServiceResult<()> {
        if password.len() < 8 {
            return Err(ServiceError::Validation(
                "password must be at least 8 characters".to_string(),
            ));
        }
        Ok(())
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
            expires_at: inv.expires_at,
            max_uses: inv.max_uses,
            use_count: inv.use_count,
            revoked_at: inv.revoked_at,
            role: inv.role,
        }
    }

    fn to_session_dto(sess: &rim_core::auth::model::Session) -> AuthSessionDto {
        AuthSessionDto {
            user_id: sess.user_id,
            session_id: sess.id,
            expires_at: sess.expires_at,
        }
    }

    /// Look up `username` and verify `password`, creating a session on success.
    ///
    /// Returns `Unauthenticated` for both unknown user and wrong password; the caller counts
    /// that outcome against the per-account login limiter.
    async fn verify_login(&self, username: &str, password: &str) -> ServiceResult<LoginResultDto> {
        let Some(user) = self.repos.user_repo.find_by_username(username).await? else {
            // Pay the same Argon2 cost as a wrong password, so response time doesn't reveal
            // whether the username exists.
            self.password_hashing.verify_dummy(password).await?;
            // Audit: failed login (user not found).
            tracing::info!(
                target: "audit",
                event = "login_failure",
                username = %username,
                reason = "user_not_found",
                "login attempt failed: user not found"
            );
            return Err(ServiceError::Unauthenticated);
        };

        // A key-only account has no password: pay the same Argon2 cost as a wrong password,
        // so neither the answer nor its timing tells it apart.
        let Some(password_hash) = user.password_hash.as_deref() else {
            self.password_hashing.verify_dummy(password).await?;
            tracing::info!(
                target: "audit",
                event = "login_failure",
                user_id = %user.id,
                username = %username,
                reason = "key_only_account",
                "login attempt failed: account has no password"
            );
            return Err(ServiceError::Unauthenticated);
        };
        let ok = self
            .password_hashing
            .verify(password, password_hash)
            .await?;
        if !ok {
            // Audit: wrong password.
            tracing::info!(
                target: "audit",
                event = "login_failure",
                user_id = %user.id,
                username = %username,
                reason = "wrong_password",
                "login attempt failed: wrong password"
            );
            return Err(ServiceError::Unauthenticated);
        }

        // An account that enrolled a key signs in with a device key only. Checked after the
        // password so the answer and its timing are the same as for a wrong password.
        if user.principal_id.is_some() {
            tracing::info!(
                target: "audit",
                event = "login_failure",
                user_id = %user.id,
                username = %username,
                reason = "key_account",
                "login attempt failed: account signs in with a device key"
            );
            return Err(ServiceError::Unauthenticated);
        }

        // An operator's complete ban blocks the account's address.
        if let Some(email) = &user.email
            && self.repos.user_repo.is_email_blocked(email).await?
        {
            tracing::info!(
                target: "audit",
                event = "blocked_email_refused",
                action = "login",
                user_id = %user.id,
                "login refused: the account's email is blocked on this server"
            );
            return Err(ServiceError::Forbidden(
                "this account is blocked on this server".to_string(),
            ));
        }

        let (sess, raw_token, csrf) = self.create_session_for_user(user.id, None, None).await?;

        // Audit: successful login.
        tracing::info!(
            target: "audit",
            event = "login_success",
            user_id = %user.id,
            session_id = %sess.id,
            "login succeeded"
        );

        Ok(LoginResultDto {
            user_id: user.id,
            session: Self::to_session_dto(&sess),
            session_token: raw_token,
            csrf_token: csrf,
        })
    }

    async fn create_session_for_user(
        &self,
        user_id: rim_core::user::model::UserId,
        user_agent: Option<String>,
        ip: Option<String>,
    ) -> ServiceResult<(rim_core::auth::model::Session, String, String)> {
        Self::create_session_in_txn(&self.repos, self.session_ttl, user_id, user_agent, ip).await
    }

    /// Insert a session row using the supplied `repos` handle.
    ///
    /// Unlike [`create_session_for_user`], this method takes an explicit
    /// [`AuthRepos`] reference so that callers can pass transactional repos.
    /// Every signup path that creates the user inside a transaction **must** use
    /// this variant for the session insert; otherwise the session INSERT runs on
    /// a separate connection that cannot see the uncommitted user row, causing
    /// an `fk-session-user` FK violation.
    ///
    /// # Transaction contract
    /// The `repos` argument **must** share the same database transaction as the
    /// user-creation call that precedes this. When both operations commit
    /// together, the FK is satisfied. If the transaction rolls back, neither the
    /// user row nor the session row is persisted.
    ///
    /// # Errors
    /// Propagates any underlying storage error from `session_repo.create_session`.
    async fn create_session_in_txn(
        repos: &AuthRepos,
        session_ttl: Duration,
        user_id: rim_core::user::model::UserId,
        user_agent: Option<String>,
        ip: Option<String>,
    ) -> ServiceResult<(rim_core::auth::model::Session, String, String)> {
        signup::create_session(
            repos.session_repo.as_ref(),
            session_ttl,
            user_id,
            user_agent,
            ip,
        )
        .await
    }

    /// Create a password account inside an open transaction (see [`signup::create_user`]).
    /// The `email` must already be normalised (lowercased, trimmed) by the caller, and
    /// `display_name` resolved.
    ///
    /// # Errors
    /// - `ServiceError::Conflict` — username already taken.
    /// - `ServiceError::EmailTaken` — email already in use (case-insensitive).
    /// - Propagates any underlying storage error.
    async fn create_user_in_txn(
        repos: &AuthRepos,
        username: &str,
        password_hash: String,
        email: &str,
        display_name: Option<String>,
    ) -> ServiceResult<rim_core::user::model::User> {
        signup::create_user(
            repos.user_repo.as_ref(),
            rim_core::user::model::NewUser {
                username: username.to_string(),
                password: Some(password_hash),
                email: Some(email.to_string()),
                display_name,
                key_identity: None,
            },
        )
        .await
    }
}

#[async_trait]
impl AuthService for DefaultAuthService {
    #[instrument(name = "auth.login", skip_all, fields(username = %cmd.username))]
    async fn login(&self, cmd: LoginCmd) -> ServiceResult<LoginResultDto> {
        let LoginCmd {
            username,
            password,
            remote_ip,
        } = cmd;
        let (username, password) = (username.as_str(), password.as_str());

        // Every limit is checked before any lookup or Argon2 verify, so a throttled caller
        // costs no hashing work and gets the same answer whether or not the username exists.
        // The strict failure limit is per (IP, username), so one source exhausting it can't
        // lock the account's owner out; the per-username limit across all IPs is a loose
        // backstop against distributed guessing (tripping it needs several addresses). A
        // source that signed in to this account recently skips the backstop, so even a
        // distributed attack can't lock the owner out of their usual address. Failure slots
        // are reserved up front (not recorded after a failure) so concurrent attempts can't
        // all pass the check before any of them fails; they are refunded below unless the
        // attempt turns out to be a failed login.
        let now = Utc::now();
        let account_key = username.to_lowercase();
        // Known sources are keyed by the exact username, which is what the lookup resolves
        // (usernames are case-sensitive); the failure limits use the folded key, which errs
        // toward sharing a budget between case variants.
        let known_source = self.login_known_sources.is_known(&remote_ip, username, now);
        let mut decision = self
            .login_ip_rate_limiter
            .check_and_record(&remote_ip, "login", now);
        if decision == RateLimitDecision::Allowed {
            decision =
                self.login_ip_account_rate_limiter
                    .check_and_record(&remote_ip, &account_key, now);
            if decision == RateLimitDecision::Allowed && !known_source {
                decision = self.login_account_rate_limiter.check_and_record(
                    LOGIN_ACCOUNT_SCOPE,
                    &account_key,
                    now,
                );
                if decision != RateLimitDecision::Allowed {
                    self.login_ip_account_rate_limiter
                        .refund(&remote_ip, &account_key, now);
                }
            }
        }
        if let RateLimitDecision::Limited {
            retry_after_seconds,
        } = decision
        {
            tracing::warn!(
                target: "audit",
                event = "login_rate_limited",
                username = %username,
                remote_ip = %remote_ip,
                retry_after_seconds = %retry_after_seconds,
                "login rate limited"
            );
            crate::metrics::rate_limited("login");
            return Err(ServiceError::RateLimited {
                retry_after_seconds,
            });
        }

        let result = self.verify_login(username, password).await;
        if !matches!(result, Err(ServiceError::Unauthenticated)) {
            self.login_ip_account_rate_limiter
                .refund(&remote_ip, &account_key, now);
            // A known source never reserved a backstop slot; refunding would free someone
            // else's slot recorded at the same instant.
            if !known_source {
                self.login_account_rate_limiter
                    .refund(LOGIN_ACCOUNT_SCOPE, &account_key, now);
            }
        }
        if result.is_ok() {
            self.login_known_sources
                .record(&remote_ip, username, Utc::now());
        }
        result
    }

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

        ensure_server_owner(
            self.repos.server_member_repo.as_ref(),
            server_id,
            actor.user_id,
        )
        .await?;

        let raw_token = generate_token("rim_inv_");
        let token_hash = hash_token(&raw_token);

        let inv = self
            .repos
            .invite_repo
            .create_invite(NewInvite {
                server_id,
                created_by_user_id: actor.user_id,
                token_hash,
                expires_at,
                max_uses,
                role,
            })
            .await?;

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

    #[instrument(name = "auth.accept_invite", skip_all, fields(username = %cmd.username))]
    async fn accept_invite(&self, cmd: AcceptInviteCmd) -> ServiceResult<AcceptInviteResultDto> {
        self.ensure_password_signup_allowed("workspace_invite")?;
        let AcceptInviteCmd {
            invite_token,
            username,
            password,
            terms_accepted_url,
        } = cmd;
        let terms_accepted_url =
            accepted_terms(self.terms_url.as_deref(), terms_accepted_url.as_deref())?;
        let (invite_token, username, password) =
            (invite_token.as_str(), username.as_str(), password.as_str());
        signup::validate_username(username)?;
        // The display name falls back to the username.
        signup::check_reserved(&self.reserved_names, username, username)?;
        Self::validate_password(password)?;

        let inv =
            signup::check_workspace_invite(self.repos.invite_repo.as_ref(), invite_token).await?;
        let server_id = inv.server_id;
        let token = signup::CheckedToken::WorkspaceInvite(inv);

        let password_hash = self.password_hashing.hash(password).await?;

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

        // Ensure username not taken (inside transaction to prevent TOCTOU race).
        if repos.user_repo.find_by_username(username).await?.is_some() {
            return Err(ServiceError::Conflict(
                "username already exists".to_string(),
            ));
        }

        // `accept_invite` takes no email (only signupWithInvite does), so supply a
        // synthetic placeholder. The user can set their email via profile-edit.
        let user = repos
            .user_repo
            .create_user(rim_core::user::model::NewUser {
                username: username.to_string(),
                password: Some(password_hash),
                email: Some(format!("{}@local.invalid", username.to_lowercase())),
                display_name: None,
                key_identity: None,
            })
            .await
            .map_err(|e| {
                // Catch unique constraint violation as a clean Conflict error
                if e.to_string().contains("unique") || e.to_string().contains("duplicate") {
                    return rim_core::repository::error::RepoError::new(
                        rim_core::repository::error::RepoErrorKind::Conflict,
                    );
                }
                e
            })?;
        signup::record_terms(repos.user_repo.as_ref(), user.id, terms_accepted_url).await?;

        // Joins the server, then consumes the invite; a failure rolls the transaction back.
        let signup_repos = repos.signup_repos();
        signup::claim_token(
            &signup_repos,
            &token,
            user.id,
            self.limits.members_per_workspace,
        )
        .await?;
        let joined_default =
            default_workspace::join(&signup_repos, self.default_workspace, user.id).await?;

        let (sess, raw_token, csrf) =
            Self::create_session_in_txn(&repos, self.session_ttl, user.id, None, None).await?;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        signup::announce_claimed(&token, user.id, username, &self.observers);
        default_workspace::announce(joined_default, user.id, username, &self.observers);

        Ok(AcceptInviteResultDto {
            user_id: user.id,
            server_id,
            session: Self::to_session_dto(&sess),
            session_token: raw_token,
            csrf_token: csrf,
        })
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
            });
        }

        info!(user_id = %actor.user_id, server_id = %inv.server_id, "user joined server via invite");

        Ok(JoinServerByInviteResultDto {
            server_id: inv.server_id,
        })
    }

    #[instrument(name = "auth.signup_with_instance_invite", skip_all, fields(username = %cmd.username))]
    async fn signup_with_instance_invite(
        &self,
        cmd: SignupWithInstanceInviteCmd,
    ) -> ServiceResult<SignupResultDto> {
        self.ensure_password_signup_allowed("instance_invite")?;
        let SignupWithInstanceInviteCmd {
            instance_invite_token: raw_token,
            username,
            password,
            email,
            display_name,
            terms_accepted_url,
        } = cmd;
        let terms_accepted_url =
            accepted_terms(self.terms_url.as_deref(), terms_accepted_url.as_deref())?;
        let (username, password) = (username.as_str(), password.as_str());

        signup::validate_username(username)?;
        Self::validate_password(password)?;
        let email = Self::validate_email(&email)?;
        let resolved_display_name =
            signup::normalize_display_name(display_name.as_deref(), username)?;
        signup::check_reserved(&self.reserved_names, username, &resolved_display_name)?;

        let token = signup::CheckedToken::InstanceInvite(
            signup::check_instance_invite(
                self.repos.instance_invite_repo.as_ref(),
                &raw_token,
                Some(&email),
            )
            .await?,
        );

        let password_hash = self.password_hashing.hash(password).await?;

        let scope = if let Some(db) = &self.db {
            Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            )
        } else {
            None
        };
        let txn_repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_auth(),
            None => self.repos.clone(),
        };

        let user = Self::create_user_in_txn(
            &txn_repos,
            username,
            password_hash,
            &email,
            Some(resolved_display_name),
        )
        .await?;
        signup::record_terms(txn_repos.user_repo.as_ref(), user.id, terms_accepted_url).await?;

        // An invite used up since the pre-check is `InvalidToken`.
        let signup_repos = txn_repos.signup_repos();
        signup::claim_token(
            &signup_repos,
            &token,
            user.id,
            self.limits.members_per_workspace,
        )
        .await?;
        let joined_default =
            default_workspace::join(&signup_repos, self.default_workspace, user.id).await?;

        let (sess, raw_sess_token, csrf) =
            Self::create_session_in_txn(&txn_repos, self.session_ttl, user.id, None, None).await?;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        signup::announce_claimed(&token, user.id, username, &self.observers);
        default_workspace::announce(joined_default, user.id, username, &self.observers);

        Ok(SignupResultDto {
            user_id: user.id,
            server_id: None,
            session: Self::to_session_dto(&sess),
            session_token: raw_sess_token,
            csrf_token: csrf,
        })
    }

    #[instrument(name = "auth.signup_open", skip_all, fields(username = %cmd.username))]
    async fn signup_open(&self, cmd: SignupOpenCmd) -> ServiceResult<SignupResultDto> {
        self.ensure_password_signup_allowed("open")?;
        let SignupOpenCmd {
            username,
            password,
            remote_ip,
            email,
            display_name,
            terms_accepted_url,
        } = cmd;
        let terms_accepted_url =
            accepted_terms(self.terms_url.as_deref(), terms_accepted_url.as_deref())?;
        let (username, password) = (username.as_str(), password.as_str());

        // Rate limit check.
        signup::check_open_signup_rate(&self.signup_rate_limiter, &remote_ip)?;

        signup::validate_username(username)?;
        Self::validate_password(password)?;
        let email = Self::validate_email(&email)?;
        let resolved_display_name =
            signup::normalize_display_name(display_name.as_deref(), username)?;
        signup::check_reserved(&self.reserved_names, username, &resolved_display_name)?;

        let password_hash = self.password_hashing.hash(password).await?;

        let scope = if let Some(db) = &self.db {
            Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            )
        } else {
            None
        };
        let txn_repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_auth(),
            None => self.repos.clone(),
        };

        let user = Self::create_user_in_txn(
            &txn_repos,
            username,
            password_hash,
            &email,
            Some(resolved_display_name),
        )
        .await?;
        signup::record_terms(txn_repos.user_repo.as_ref(), user.id, terms_accepted_url).await?;
        let joined_default =
            default_workspace::join(&txn_repos.signup_repos(), self.default_workspace, user.id)
                .await?;
        let (sess, raw_sess_token, csrf) =
            Self::create_session_in_txn(&txn_repos, self.session_ttl, user.id, None, None).await?;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        default_workspace::announce(joined_default, user.id, username, &self.observers);
        // Audit: successful open-mode signup.
        tracing::info!(
            target: "audit",
            event = "signup_open_success",
            user_id = %user.id,
            remote_ip = %remote_ip,
            "user created via open signup"
        );
        info!(user_id = %user.id, "user created via open signup");

        Ok(SignupResultDto {
            user_id: user.id,
            server_id: None,
            session: Self::to_session_dto(&sess),
            session_token: raw_sess_token,
            csrf_token: csrf,
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
}
