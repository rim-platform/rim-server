use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use sea_orm::DatabaseConnection;
use tracing::{info, instrument, warn};

use rim_auth::csrf::generate_csrf_token;
use rim_auth::password::{hash_password, verify_password};
use rim_auth::session_token::{generate_session_token, hash_token};
use rim_core::auth::model::{NewInstanceInvite, NewInvite, NewSession};
use rim_core::conversation::model::ConversationType;
use rim_core::server_member::model::ServerMemberRole;
use rim_infra_db::begin_transaction;
use rim_services::auth::command::{
    AcceptInviteCmd, BootstrapCmd, CreateInstanceInviteCmd, CreateInviteCmd, JoinServerByInviteCmd,
    ListInvitesQuery, LoginCmd, LogoutCmd, PreviewInviteTokenQuery, RevokeInstanceInviteCmd,
    RevokeInviteCmd, SignupOpenCmd, SignupWithBootstrapCmd, SignupWithInstanceInviteCmd,
};
use rim_services::auth::dto::{
    AcceptInviteResultDto, AuthSessionDto, BootstrapResultDto, CreateInstanceInviteResultDto,
    CreateInviteResultDto, InstanceInviteDto, InviteDto, InviteTokenPreviewDto, InviteTokenType,
    JoinServerByInviteResultDto, LoginResultDto, SignupResultDto, WorkspaceInvitePreviewDto,
};
use rim_services::auth::service::AuthService;
use rim_services::{Actor, ServiceError, ServiceResult};

use rim_services::events::{EventObserver, Observers, WorkspaceEvent};

use crate::AuthRepos;
use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::repos::AppRepos;
use crate::util::ensure_server_owner;

#[derive(Clone)]
pub struct DefaultAuthService {
    repos: AuthRepos,
    db: Option<DatabaseConnection>,
    bootstrap_secret: Option<String>,
    session_ttl: Duration,
    observers: Observers,
    preview_rate_limiter: RegistrationRateLimiter,
    signup_rate_limiter: RegistrationRateLimiter,
}

impl DefaultAuthService {
    /// Construct a new auth service with default rate limiters.
    ///
    /// # Examples
    /// ```ignore
    /// let svc = DefaultAuthService::new(repos, Some(db), None, Duration::days(30), observers);
    /// ```
    pub fn new(
        repos: AuthRepos,
        db: Option<DatabaseConnection>,
        bootstrap_secret: Option<String>,
        session_ttl: Duration,
        observers: Observers,
    ) -> Self {
        Self {
            repos,
            db,
            bootstrap_secret,
            session_ttl,
            observers,
            preview_rate_limiter: RegistrationRateLimiter::preview_default(),
            signup_rate_limiter: RegistrationRateLimiter::signup_default(),
        }
    }

    /// Access the preview rate limiter — primarily useful for testing / introspection.
    #[allow(dead_code)]
    pub fn preview_rate_limiter(&self) -> &RegistrationRateLimiter {
        &self.preview_rate_limiter
    }

    /// Access the signup rate limiter — primarily useful for testing / introspection.
    #[allow(dead_code)]
    pub fn signup_rate_limiter(&self) -> &RegistrationRateLimiter {
        &self.signup_rate_limiter
    }

    /// Construct with explicit rate limiters — for testing or custom deployments.
    ///
    /// # Examples
    /// ```ignore
    /// let svc = DefaultAuthService::with_rate_limiters(
    ///     repos, Some(db), None, Duration::days(30), observers,
    ///     preview_rl, signup_rl,
    /// );
    /// ```
    #[allow(dead_code)]
    pub fn with_rate_limiters(
        repos: AuthRepos,
        db: Option<DatabaseConnection>,
        bootstrap_secret: Option<String>,
        session_ttl: Duration,
        observers: Observers,
        preview_rate_limiter: RegistrationRateLimiter,
        signup_rate_limiter: RegistrationRateLimiter,
    ) -> Self {
        Self {
            repos,
            db,
            bootstrap_secret,
            session_ttl,
            observers,
            preview_rate_limiter,
            signup_rate_limiter,
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
        }
    }

    fn to_session_dto(sess: &rim_core::auth::model::Session) -> AuthSessionDto {
        AuthSessionDto {
            user_id: sess.user_id,
            session_id: sess.id,
            expires_at: sess.expires_at,
        }
    }

    async fn create_session_for_user(
        &self,
        user_id: rim_core::user::model::UserId,
        user_agent: Option<String>,
        ip: Option<String>,
    ) -> ServiceResult<(rim_core::auth::model::Session, String, String)> {
        let raw_token = generate_session_token();
        let token_hash = hash_token(&raw_token);
        let expires_at: DateTime<Utc> = Utc::now() + self.session_ttl;
        let sess = self
            .repos
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

    async fn ensure_no_users_exist(&self) -> ServiceResult<()> {
        let exists = self.repos.user_repo.any_users_exist().await?;
        if exists {
            return Err(ServiceError::PermissionDenied);
        }
        Ok(())
    }

    /// Create a new user inside an open transaction — checks for username conflicts first.
    ///
    /// Returns [`ServiceError::Conflict`] if the username is already taken.
    async fn create_user_in_txn(
        repos: &AuthRepos,
        username: &str,
        password_hash: String,
    ) -> ServiceResult<rim_core::user::model::User> {
        if repos.user_repo.find_by_username(username).await?.is_some() {
            return Err(ServiceError::Conflict(
                "username already exists".to_string(),
            ));
        }

        let user = repos
            .user_repo
            .create_user(rim_core::user::model::NewUser {
                username: username.to_string(),
                password: password_hash,
            })
            .await
            .map_err(|e| {
                if e.to_string().contains("unique") || e.to_string().contains("duplicate") {
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
            {
                if member.member_role == ServerMemberRole::Owner {
                    return Ok(());
                }
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
    #[instrument(name = "auth.bootstrap", skip_all, fields(username = %cmd.username))]
    async fn bootstrap(&self, cmd: BootstrapCmd) -> ServiceResult<BootstrapResultDto> {
        let BootstrapCmd {
            secret,
            username,
            password,
            server_name,
        } = cmd;
        let (secret, username, password, server_name) = (
            secret.as_str(),
            username.as_str(),
            password.as_str(),
            server_name.as_str(),
        );
        info!(username = %username, "check bootstrap secret");
        let expected = self
            .bootstrap_secret
            .as_deref()
            .ok_or(ServiceError::PermissionDenied)?;
        if secret != expected {
            return Err(ServiceError::PermissionDenied);
        }

        self.ensure_no_users_exist().await?;

        Self::validate_username(username)?;
        Self::validate_password(password)?;

        info!(username = %username, "hash the password");
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

        info!(username = %username, "add user");
        let user = repos
            .user_repo
            .create_user(rim_core::user::model::NewUser {
                username: username.to_string(),
                password: password_hash,
            })
            .await
            .map_err(|e| {
                tracing::error!(error = %e, kind = ?e.kind(), "create_user failed");
                ServiceError::from(e)
            })?;

        info!(username = %username, "add server");
        let server = repos
            .server_repo
            .create_server(server_name)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, kind = ?e.kind(), "create_server failed");
                ServiceError::from(e)
            })?;

        info!(username = %username, "add member");
        let _member = repos
            .server_member_repo
            .add_member(server.id, user.id, ServerMemberRole::Owner)
            .await?;

        // Create default #general and #random channels.
        for channel_name in &["general", "random"] {
            match repos
                .conversation_repo
                .create_conversation_with_members(
                    ConversationType::Channel,
                    server.id,
                    Some(channel_name.to_string()),
                    &[user.id],
                )
                .await
            {
                Ok(conv) => {
                    info!(
                        server_id = %server.id,
                        conversation_id = %conv.id,
                        channel = %channel_name,
                        "created default channel"
                    );
                }
                Err(err) => {
                    warn!(
                        server_id = %server.id,
                        channel = %channel_name,
                        error = %err,
                        "failed to create default channel during bootstrap"
                    );
                }
            }
        }

        info!(username = %username, "create session");
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

        let csrf = Self::new_csrf_token(&raw_token);

        info!(user_id = %user.id, server_id = %server.id, "bootstrapped instance");

        Ok(BootstrapResultDto {
            user_id: user.id,
            server_id: server.id,
            session: Self::to_session_dto(&sess),
            session_token: raw_token,
            csrf_token: csrf,
        })
    }

    #[instrument(name = "auth.login", skip_all, fields(username = %cmd.username))]
    async fn login(&self, cmd: LoginCmd) -> ServiceResult<LoginResultDto> {
        let LoginCmd { username, password } = cmd;
        let (username, password) = (username.as_str(), password.as_str());
        let user = self
            .repos
            .user_repo
            .find_by_username(username)
            .await?
            .ok_or(ServiceError::Unauthenticated)?;

        let ok = verify_password(password, &user.password_hash).map_err(ServiceError::internal)?;
        if !ok {
            return Err(ServiceError::Unauthenticated);
        }

        let (sess, raw_token, csrf) = self.create_session_for_user(user.id, None, None).await?;

        Ok(LoginResultDto {
            user_id: user.id,
            session: Self::to_session_dto(&sess),
            session_token: raw_token,
            csrf_token: csrf,
        })
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
        } = cmd;
        if max_uses <= 0 {
            return Err(ServiceError::Validation("max_uses must be > 0".to_string()));
        }

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
            })
            .await?;

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
        if let Some(expires_at) = inv.expires_at {
            if expires_at < Utc::now() {
                return Err(ServiceError::PermissionDenied);
            }
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

        let user = repos
            .user_repo
            .create_user(rim_core::user::model::NewUser {
                username: username.to_string(),
                password: password_hash,
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

        let _member = repos
            .server_member_repo
            .add_member(inv.server_id, user.id, ServerMemberRole::Guest)
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
        if let Some(expires_at) = inv.expires_at {
            if expires_at < Utc::now() {
                return Err(ServiceError::PermissionDenied);
            }
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

        let _member = repos
            .server_member_repo
            .add_member(inv.server_id, actor.user_id, ServerMemberRole::Guest)
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
        } = cmd;
        let (username, password) = (username.as_str(), password.as_str());

        Self::validate_username(username)?;
        Self::validate_password(password)?;

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

        let user = Self::create_user_in_txn(&txn_repos, username, password_hash).await?;

        txn_repos
            .bootstrap_token_repo
            .mark_used(token.id, user.id)
            .await?;

        let (sess, raw_sess_token, csrf) =
            self.create_session_for_user(user.id, None, None).await?;

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
        } = cmd;
        let (username, password) = (username.as_str(), password.as_str());

        Self::validate_username(username)?;
        Self::validate_password(password)?;

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

        let user = Self::create_user_in_txn(&txn_repos, username, password_hash).await?;

        let consumed = txn_repos
            .instance_invite_repo
            .try_consume(invite.id)
            .await?;
        if !consumed {
            // Exhausted since we last checked — treat as invalid.
            return Err(ServiceError::InvalidToken);
        }

        let (sess, raw_sess_token, csrf) =
            self.create_session_for_user(user.id, None, None).await?;

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
            return Err(ServiceError::RateLimited {
                retry_after_seconds,
            });
        }

        Self::validate_username(username)?;
        Self::validate_password(password)?;

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

        let user = Self::create_user_in_txn(&txn_repos, username, password_hash).await?;
        let (sess, raw_sess_token, csrf) =
            self.create_session_for_user(user.id, None, None).await?;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

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
                token_type: InviteTokenType::Instance,
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
            if let Some(expires_at) = inv.expires_at {
                if expires_at < Utc::now() {
                    return Err(ServiceError::ExpiredToken);
                }
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
