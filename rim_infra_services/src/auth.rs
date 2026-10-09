use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use sea_orm::DatabaseConnection;
use std::sync::OnceLock;
use tracing::{info, instrument, warn};

use rim_auth::csrf::generate_csrf_token;
use rim_auth::password::{hash_password, verify_dummy_password, verify_password};
use rim_auth::session_token::{generate_session_token, hash_token};
use rim_core::auth::model::{NewInstanceInvite, NewInvite, NewSession};
use rim_core::server_member::model::ServerMemberRole;
use rim_infra_db::begin_transaction;
use rim_services::auth::command::{
    AcceptInviteCmd, CreateInstanceInviteCmd, CreateInviteCmd, JoinServerByInviteCmd,
    ListInvitesQuery, LoginCmd, LogoutCmd, PreviewInviteTokenQuery, RevokeInstanceInviteCmd,
    RevokeInviteCmd, SignupOpenCmd, SignupWithBootstrapCmd, SignupWithInstanceInviteCmd,
};
use rim_services::auth::dto::{
    AcceptInviteResultDto, AuthSessionDto, CreateInstanceInviteResultDto, CreateInviteResultDto,
    InstanceInviteDto, InviteDto, InviteTokenPreviewDto, InviteTokenType,
    JoinServerByInviteResultDto, LoginResultDto, SignupResultDto, WorkspaceInvitePreviewDto,
};
use rim_services::auth::service::AuthService;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::{Actor, ServiceError, ServiceResult};

use rim_services::events::{EventObserver, Observers, WorkspaceEvent};

use crate::AuthRepos;
use crate::email::{EmailSender, templates};
use crate::login_sources::KnownLoginSources;
use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::repos::AppRepos;
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
    /// invite emails fall back to the bare token (dev-only — see `docs/email.md`).
    public_base_url: Option<String>,
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
        }
    }

    /// Build the invite link as a path segment: `{public_base_url}/join/{token}` — never a
    /// `?token=` query string (see `docs/email.md`). Falls back to the bare token (with a
    /// warning) if `public_base_url` is unset.
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
    ///
    /// # Examples
    /// ```ignore
    /// let svc = DefaultAuthService::new(/* ... */).with_login_rate_limiters(
    ///     RegistrationRateLimiter::new(300, 30),
    ///     RegistrationRateLimiter::new(900, 10),
    ///     RegistrationRateLimiter::new(3600, 100),
    /// );
    /// ```
    #[allow(dead_code)]
    pub fn with_login_rate_limiters(
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

    /// Share the store of recent successful login sources, so other services (password
    /// reset) can forget an account's sources.
    pub(crate) fn with_known_login_sources(mut self, sources: KnownLoginSources) -> Self {
        self.login_known_sources = sources;
        self
    }

    /// Construct with explicit rate limiters — for testing or custom deployments.
    ///
    /// # Examples
    /// ```ignore
    /// let svc = DefaultAuthService::with_rate_limiters(
    ///     repos, Some(db), Duration::days(30), observers,
    ///     preview_rl, signup_rl, email_sender, None,
    /// );
    /// ```
    #[allow(dead_code, clippy::too_many_arguments)]
    pub fn with_rate_limiters(
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

    fn validate_username(username: &str) -> ServiceResult<()> {
        let trimmed = username.trim();
        let len = trimmed.len();
        if !(3..=32).contains(&len) {
            return Err(ServiceError::Validation(
                "username must be between 3 and 32 characters".to_string(),
            ));
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(ServiceError::Validation(
                "username may only contain alphanumeric characters, underscores, or hyphens"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Validate and normalise an email address.
    ///
    /// Trims outer whitespace, lowercases, checks the pragmatic regex from spec
    /// §13.3, and enforces a 1–254 character length. Returns the normalised
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

    /// Normalise an optional display name, falling back to `username` when
    /// absent or blank after trim.
    ///
    /// Length of a non-empty (post-trim) value is capped at 80 characters per
    /// spec §13.3. An 81-character value returns a `Validation` error.
    ///
    /// # Errors
    /// - `ServiceError::Validation("display name length")` if the trimmed value
    ///   is non-empty and its length exceeds 80 characters.
    ///
    /// # Examples
    /// ```ignore
    /// let name = DefaultAuthService::normalize_display_name(None, "alice")?;
    /// assert_eq!(name, "alice");
    ///
    /// let name = DefaultAuthService::normalize_display_name(Some("   "), "alice")?;
    /// assert_eq!(name, "alice");
    ///
    /// let name = DefaultAuthService::normalize_display_name(Some("Alice Smith"), "alice")?;
    /// assert_eq!(name, "Alice Smith");
    /// ```
    pub(crate) fn normalize_display_name(
        input: Option<&str>,
        username: &str,
    ) -> ServiceResult<String> {
        match input {
            None => Ok(username.to_string()),
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Ok(username.to_string());
                }
                if trimmed.len() > 80 {
                    return Err(ServiceError::Validation("display name length".to_string()));
                }
                Ok(trimmed.to_string())
            }
        }
    }

    fn new_csrf_token(session_token: &str) -> String {
        generate_csrf_token(session_token)
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
            verify_dummy_password(password).map_err(ServiceError::internal)?;
            // H4 audit: log failed login (user not found).
            tracing::info!(
                target = "audit",
                event = "login_failure",
                username = %username,
                reason = "user_not_found",
                "login attempt failed: user not found"
            );
            return Err(ServiceError::Unauthenticated);
        };

        let ok = verify_password(password, &user.password_hash).map_err(ServiceError::internal)?;
        if !ok {
            // H4 audit: wrong password.
            tracing::info!(
                target = "audit",
                event = "login_failure",
                user_id = %user.id,
                username = %username,
                reason = "wrong_password",
                "login attempt failed: wrong password"
            );
            return Err(ServiceError::Unauthenticated);
        }

        // An account that enrolled a key signs in with a device key only (key-identity spec,
        // "Password sign-in"). Checked after the password so the answer and its timing are the
        // same as for a wrong password.
        if user.principal_id.is_some() {
            tracing::info!(
                target = "audit",
                event = "login_failure",
                user_id = %user.id,
                username = %username,
                reason = "key_account",
                "login attempt failed: account signs in with a device key"
            );
            return Err(ServiceError::Unauthenticated);
        }

        let (sess, raw_token, csrf) = self.create_session_for_user(user.id, None, None).await?;

        // H4 audit: successful login.
        tracing::info!(
            target = "audit",
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
        let raw_token = generate_session_token();
        let token_hash = hash_token(&raw_token);
        let expires_at: DateTime<Utc> = Utc::now() + session_ttl;
        let sess = repos
            .session_repo
            .create_session(NewSession {
                user_id,
                token_hash,
                expires_at,
                user_agent,
                ip,
            })
            .await?;

        let csrf = Self::new_csrf_token(&raw_token);
        Ok((sess, raw_token, csrf))
    }

    /// Create a new user inside an open transaction.
    ///
    /// Checks username uniqueness, then email uniqueness (case-insensitive),
    /// before inserting. The `email` must already be normalised (lowercased,
    /// trimmed) by the caller. `display_name` is stored as-is (the fallback to
    /// username must be resolved before this call).
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
        if repos.user_repo.find_by_username(username).await?.is_some() {
            return Err(ServiceError::Conflict(
                "username already exists".to_string(),
            ));
        }

        if repos.user_repo.find_by_email(email).await?.is_some() {
            // H4 audit: log email-taken attempts so operators can detect
            // enumeration attacks or signup bot activity.
            tracing::info!(
                target = "audit",
                event = "email_taken_attempt",
                email = %email,
                username = %username,
                "signup rejected: email already in use"
            );
            return Err(ServiceError::EmailTaken);
        }

        let user = repos
            .user_repo
            .create_user(rim_core::user::model::NewUser {
                username: username.to_string(),
                password: password_hash,
                email: email.to_string(),
                display_name,
            })
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("unique") || msg.contains("duplicate") {
                    return rim_core::repository::error::RepoError::new(
                        rim_core::repository::error::RepoErrorKind::Conflict,
                    );
                }
                e
            })?;

        Ok(user)
    }

    /// Ensure the actor is an Owner on at least one server (instance-invite gate).
    async fn ensure_any_server_owner(
        repos: &AuthRepos,
        actor_user_id: rim_core::user::model::UserId,
    ) -> ServiceResult<()> {
        let servers = repos
            .server_repo
            .get_servers_for_user(actor_user_id)
            .await?;
        for server in &servers {
            if let Some(member) = repos
                .server_member_repo
                .get_member(server.id, actor_user_id)
                .await?
                && member.member_role == ServerMemberRole::Owner
            {
                return Ok(());
            }
        }
        Err(ServiceError::PermissionDenied)
    }

    fn generate_invite_token_with_prefix(prefix: &str) -> String {
        let t = generate_session_token();
        t.strip_prefix("rim_sess_")
            .map(|s| format!("{prefix}{s}"))
            .unwrap_or_else(|| format!("{prefix}{t}"))
    }

    fn to_instance_invite_dto(inv: rim_core::auth::model::InstanceInvite) -> InstanceInviteDto {
        InstanceInviteDto {
            id: inv.id,
            created_by_user_id: inv.created_by_user_id,
            expires_at: inv.expires_at,
            max_uses: inv.max_uses,
            use_count: inv.use_count,
            revoked_at: inv.revoked_at,
            created_at: inv.created_at,
        }
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
                target = "audit",
                event = "login_rate_limited",
                username = %username,
                remote_ip = %remote_ip,
                retry_after_seconds = %retry_after_seconds,
                "login rate limited"
            );
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

        Ok(())
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
        if max_uses <= 0 {
            return Err(ServiceError::Validation("max_uses must be > 0".to_string()));
        }
        // An invite grants `Member` or the restricted `Guest`, never a moderating role.
        if !matches!(role, ServerMemberRole::Member | ServerMemberRole::Guest) {
            return Err(ServiceError::Validation(
                "an invite can grant only the member or guest role".to_string(),
            ));
        }

        // Validate before persisting, per docs/email.md — a malformed address should surface
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

        let raw_token = {
            let t = generate_session_token();
            t.strip_prefix("rim_sess_")
                .map(|s| format!("rim_inv_{s}"))
                .unwrap_or_else(|| format!("rim_inv_{t}"))
        };
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
        let AcceptInviteCmd {
            invite_token,
            username,
            password,
        } = cmd;
        let (invite_token, username, password) =
            (invite_token.as_str(), username.as_str(), password.as_str());
        Self::validate_username(username)?;
        Self::validate_password(password)?;

        let token_hash = hash_token(invite_token);
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

        let password_hash = hash_password(password).map_err(ServiceError::internal)?;

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

        // `accept_invite` predates the email requirement (Q2 scope decision: email
        // is added only to signupWithInvite, not acceptInvite). Supply a synthetic
        // placeholder. The user must update their email via profile-edit.
        let user = repos
            .user_repo
            .create_user(rim_core::user::model::NewUser {
                username: username.to_string(),
                password: password_hash,
                email: format!("{}@local.invalid", username.to_lowercase()),
                display_name: None,
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

        // A brand-new account cannot carry a ban yet; checked anyway so every join path
        // refuses banned users the same way (Track 1.2).
        ensure_not_banned(repos.moderation_repo.as_ref(), inv.server_id, user.id).await?;
        let _member = add_workspace_member(
            repos.server_member_repo.as_ref(),
            repos.conversation_repo.as_ref(),
            repos.conversation_member_repo.as_ref(),
            inv.server_id,
            user.id,
            inv.role,
        )
        .await?;

        let consumed = repos.invite_repo.try_consume(inv.id).await?;
        if !consumed {
            // Don't leak whether it's exhausted vs missing.
            // Transaction is dropped → rolled back automatically.
            return Err(ServiceError::PermissionDenied);
        }

        let raw_token = generate_session_token();
        let token_hash = hash_token(&raw_token);
        let expires_at: DateTime<Utc> = Utc::now() + self.session_ttl;
        let sess = repos
            .session_repo
            .create_session(NewSession {
                user_id: user.id,
                token_hash,
                expires_at,
                user_agent: None,
                ip: None,
            })
            .await?;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        self.observers.emit(WorkspaceEvent::MemberJoined {
            workspace_id: inv.server_id,
            user_id: user.id,
            username: username.to_string(),
        });

        // H4 audit: workspace-invite signup success.
        tracing::info!(
            target = "audit",
            event = "accept_invite_success",
            user_id = %user.id,
            invite_id = %inv.id,
            server_id = %inv.server_id,
            "user created via workspace invite"
        );

        let csrf = Self::new_csrf_token(&raw_token);

        Ok(AcceptInviteResultDto {
            user_id: user.id,
            server_id: inv.server_id,
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
        // Banned users cannot re-join (Track 1.2). Checked before the invite is consumed.
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

    #[instrument(name = "auth.list_invites", skip_all, fields(actor_id = %query.actor.user_id, server_id = %query.server_id))]
    async fn list_invites(&self, query: ListInvitesQuery) -> ServiceResult<Vec<InviteDto>> {
        ensure_server_owner(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let invites = self
            .repos
            .invite_repo
            .list_active_by_server(query.server_id)
            .await?;

        Ok(invites.into_iter().map(Self::to_invite_dto).collect())
    }

    #[instrument(name = "auth.revoke_invite", skip_all, fields(actor_id = %cmd.actor.user_id, invite_id = %cmd.invite_id))]
    async fn revoke_invite(&self, cmd: RevokeInviteCmd) -> ServiceResult<()> {
        let inv = self
            .repos
            .invite_repo
            .get_by_id(cmd.invite_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        ensure_server_owner(
            self.repos.server_member_repo.as_ref(),
            inv.server_id,
            cmd.actor.user_id,
        )
        .await?;

        self.repos.invite_repo.revoke_invite(cmd.invite_id).await?;

        // H4 audit: workspace invite revocation.
        tracing::info!(
            target = "audit",
            event = "invite_revoked",
            invite_id = %cmd.invite_id,
            actor_user_id = %cmd.actor.user_id,
            "workspace invite revoked"
        );
        info!(invite_id = %cmd.invite_id, "invite revoked");
        Ok(())
    }

    #[instrument(name = "auth.signup_with_bootstrap", skip_all, fields(username = %cmd.username))]
    async fn signup_with_bootstrap(
        &self,
        cmd: SignupWithBootstrapCmd,
    ) -> ServiceResult<SignupResultDto> {
        let SignupWithBootstrapCmd {
            bootstrap_token: raw_token,
            username,
            password,
            email,
            display_name,
        } = cmd;
        let (username, password) = (username.as_str(), password.as_str());

        Self::validate_username(username)?;
        Self::validate_password(password)?;
        let email = Self::validate_email(&email)?;
        let resolved_display_name =
            Self::normalize_display_name(display_name.as_deref(), username)?;

        let token_hash = hash_token(&raw_token);
        let token = self
            .repos
            .bootstrap_token_repo
            .get_by_token_hash(&token_hash)
            .await?
            .ok_or(ServiceError::InvalidToken)?;

        // Indistinguishable: consumed and revoked both map to InvalidToken (§4.5).
        if token.used_at.is_some() || token.revoked_at.is_some() {
            return Err(ServiceError::InvalidToken);
        }
        if token.expires_at < Utc::now() {
            return Err(ServiceError::ExpiredToken);
        }

        let password_hash = hash_password(password).map_err(ServiceError::internal)?;

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

        // Atomic try-mark: eliminates the double-spend race (C1 hardening).
        // If another concurrent request already consumed the token between our
        // pre-check and this UPDATE, rows_affected == 0 → return InvalidToken
        // and roll back the transaction.
        let claimed = txn_repos
            .bootstrap_token_repo
            .try_mark_used(token.id, user.id)
            .await?;
        if !claimed {
            // Rollback is automatic when `scope` is dropped without commit.
            tracing::info!(
                target = "audit",
                event = "bootstrap_token_double_spend_rejected",
                bootstrap_token_id = %token.id,
                "bootstrap token already consumed by concurrent request"
            );
            return Err(ServiceError::InvalidToken);
        }

        let (sess, raw_sess_token, csrf) =
            Self::create_session_in_txn(&txn_repos, self.session_ttl, user.id, None, None).await?;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        tracing::info!(
            target = "audit",
            event = "bootstrap_token_consumed",
            bootstrap_token_id = %token.id,
            consumed_by_user_id = %user.id,
            "bootstrap token consumed"
        );

        info!(user_id = %user.id, "user created via bootstrap token");

        Ok(SignupResultDto {
            user_id: user.id,
            server_id: None,
            session: Self::to_session_dto(&sess),
            session_token: raw_sess_token,
            csrf_token: csrf,
        })
    }

    #[instrument(name = "auth.signup_with_instance_invite", skip_all, fields(username = %cmd.username))]
    async fn signup_with_instance_invite(
        &self,
        cmd: SignupWithInstanceInviteCmd,
    ) -> ServiceResult<SignupResultDto> {
        let SignupWithInstanceInviteCmd {
            instance_invite_token: raw_token,
            username,
            password,
            email,
            display_name,
        } = cmd;
        let (username, password) = (username.as_str(), password.as_str());

        Self::validate_username(username)?;
        Self::validate_password(password)?;
        let email = Self::validate_email(&email)?;
        let resolved_display_name =
            Self::normalize_display_name(display_name.as_deref(), username)?;

        let token_hash = hash_token(&raw_token);
        let invite = self
            .repos
            .instance_invite_repo
            .get_by_token_hash(&token_hash)
            .await?
            .ok_or(ServiceError::InvalidToken)?;

        if invite.revoked_at.is_some() {
            return Err(ServiceError::InvalidToken);
        }
        if invite.expires_at < Utc::now() {
            return Err(ServiceError::ExpiredToken);
        }

        let password_hash = hash_password(password).map_err(ServiceError::internal)?;

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

        let consumed = txn_repos
            .instance_invite_repo
            .try_consume(invite.id)
            .await?;
        if !consumed {
            // Exhausted since we last checked — treat as invalid.
            return Err(ServiceError::InvalidToken);
        }

        let (sess, raw_sess_token, csrf) =
            Self::create_session_in_txn(&txn_repos, self.session_ttl, user.id, None, None).await?;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        tracing::info!(
            target = "audit",
            event = "instance_invite_consumed",
            instance_invite_id = %invite.id,
            consumed_by_user_id = %user.id,
            "instance invite consumed"
        );

        info!(user_id = %user.id, "user created via instance invite");

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
        let SignupOpenCmd {
            username,
            password,
            remote_ip,
            email,
            display_name,
        } = cmd;
        let (username, password) = (username.as_str(), password.as_str());

        // Rate limit check (§4.4).
        let decision = self
            .signup_rate_limiter
            .check_and_record(&remote_ip, "signup", Utc::now());
        if let rim_services::command::rate_limit::RateLimitDecision::Limited {
            retry_after_seconds,
        } = decision
        {
            // H4 audit: rate-limited signup attempts for operator visibility.
            tracing::info!(
                target = "audit",
                event = "signup_rate_limited",
                remote_ip = %remote_ip,
                retry_after_seconds = %retry_after_seconds,
                "open-mode signup rate limited"
            );
            return Err(ServiceError::RateLimited {
                retry_after_seconds,
            });
        }

        Self::validate_username(username)?;
        Self::validate_password(password)?;
        let email = Self::validate_email(&email)?;
        let resolved_display_name =
            Self::normalize_display_name(display_name.as_deref(), username)?;

        let password_hash = hash_password(password).map_err(ServiceError::internal)?;

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
        let (sess, raw_sess_token, csrf) =
            Self::create_session_in_txn(&txn_repos, self.session_ttl, user.id, None, None).await?;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        // H4 audit: log successful open-mode signup.
        tracing::info!(
            target = "audit",
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

    #[instrument(name = "auth.create_instance_invite", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn create_instance_invite(
        &self,
        cmd: CreateInstanceInviteCmd,
    ) -> ServiceResult<CreateInstanceInviteResultDto> {
        // Gate: must be Owner of at least one server (§5 placeholder gate).
        Self::ensure_any_server_owner(&self.repos, cmd.actor.user_id).await?;

        // Validate before persisting, per docs/email.md — a malformed address should surface
        // as a validation error, not a silently-dropped invite email.
        let recipient_email = cmd
            .recipient_email
            .clone()
            .map(|email| Self::validate_email(&email))
            .transpose()?;

        // Validate expires_at: mandatory, must be in the future, at most 30 days out.
        let expires_at = cmd
            .expires_at
            .unwrap_or_else(|| Utc::now() + Duration::days(7));
        let now = Utc::now();
        if expires_at <= now {
            return Err(ServiceError::Validation(
                "expires_at must be in the future".to_string(),
            ));
        }
        if expires_at > now + Duration::days(30) {
            return Err(ServiceError::Validation(
                "expires_at may not be more than 30 days from now".to_string(),
            ));
        }

        let max_uses = cmd.max_uses.unwrap_or(1);
        if max_uses <= 0 || max_uses > 50 {
            return Err(ServiceError::Validation(
                "max_uses must be between 1 and 50 inclusive".to_string(),
            ));
        }

        let raw_token = Self::generate_invite_token_with_prefix("rim_iinv_");
        let token_hash = hash_token(&raw_token);

        let invite = self
            .repos
            .instance_invite_repo
            .create(NewInstanceInvite {
                created_by_user_id: cmd.actor.user_id,
                token_hash,
                expires_at,
                max_uses,
            })
            .await?;

        tracing::info!(
            target = "audit",
            event = "instance_invite_issued",
            instance_invite_id = %invite.id,
            issued_by_user_id = %cmd.actor.user_id,
            "instance invite issued"
        );

        if let Some(recipient_email) = recipient_email {
            let link = self.build_invite_link(&raw_token);
            let message = templates::invite(&recipient_email, &link);
            if let Err(e) = self.email_sender.send(message).await {
                warn!(error = %e, "failed to send instance invite email");
            }
        }

        Ok(CreateInstanceInviteResultDto {
            invite: Self::to_instance_invite_dto(invite),
            invite_token: raw_token,
        })
    }

    #[instrument(name = "auth.revoke_instance_invite", skip_all, fields(actor_id = %cmd.actor.user_id, invite_id = %cmd.invite_id))]
    async fn revoke_instance_invite(&self, cmd: RevokeInstanceInviteCmd) -> ServiceResult<()> {
        Self::ensure_any_server_owner(&self.repos, cmd.actor.user_id).await?;

        let _invite = self
            .repos
            .instance_invite_repo
            .get_by_id(cmd.invite_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        self.repos
            .instance_invite_repo
            .revoke(cmd.invite_id)
            .await?;

        // H4 audit: instance invite revocation.
        tracing::info!(
            target = "audit",
            event = "instance_invite_revoked",
            invite_id = %cmd.invite_id,
            actor_user_id = %cmd.actor.user_id,
            "instance invite revoked"
        );
        info!(invite_id = %cmd.invite_id, "instance invite revoked");
        Ok(())
    }

    #[instrument(name = "auth.list_instance_invites", skip_all, fields(actor_id = %actor.user_id))]
    async fn list_instance_invites(&self, actor: Actor) -> ServiceResult<Vec<InstanceInviteDto>> {
        Self::ensure_any_server_owner(&self.repos, actor.user_id).await?;
        let invites = self.repos.instance_invite_repo.list_active().await?;
        Ok(invites
            .into_iter()
            .map(Self::to_instance_invite_dto)
            .collect())
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
                target = "audit",
                event = "preview_invite_token_rate_limited",
                remote_ip = %query.remote_ip,
                "preview_invite_token rate limit exceeded"
            );
            return Err(ServiceError::RateLimited {
                retry_after_seconds,
            });
        }

        let token_hash = hash_token(&query.token);

        // 1. Check bootstrap_token first.
        if let Some(bt) = self
            .repos
            .bootstrap_token_repo
            .get_by_token_hash(&token_hash)
            .await?
        {
            if bt.used_at.is_some() || bt.revoked_at.is_some() {
                return Err(ServiceError::InvalidToken);
            }
            if bt.expires_at < Utc::now() {
                return Err(ServiceError::ExpiredToken);
            }
            return Ok(InviteTokenPreviewDto {
                token_type: InviteTokenType::Bootstrap,
                expires_at: Some(bt.expires_at),
                workspace: None,
            });
        }

        // 2. Check instance_invite.
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

        // 3. Check workspace invite.
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

            // Fetch workspace name and inviter display name.
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

            return Ok(InviteTokenPreviewDto {
                token_type: InviteTokenType::Workspace,
                expires_at: inv.expires_at,
                workspace: Some(WorkspaceInvitePreviewDto {
                    server_id: inv.server_id,
                    server_name: server.name,
                    invited_by_display_name: inviter_display,
                }),
            });
        }

        Err(ServiceError::InvalidToken)
    }
}
