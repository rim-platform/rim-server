use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use sea_orm::DatabaseConnection;
use tracing::{info, instrument, warn};

use rim_auth::password::{hash_password, verify_password};
use rim_auth::session_token::{generate_session_token, hash_token};
use rim_core::auth::model::{NewInvite, NewSession};
use rim_core::conversation::model::ConversationType;
use rim_core::server_member::model::ServerMemberRole;
use rim_infra_db::begin_transaction;
use rim_services::auth::command::{
    AcceptInviteCmd, BootstrapCmd, CreateInviteCmd, JoinServerByInviteCmd, ListInvitesQuery,
    LoginCmd, LogoutCmd, RevokeInviteCmd,
};
use rim_services::auth::dto::{
    AcceptInviteResultDto, AuthSessionDto, BootstrapResultDto, CreateInviteResultDto, InviteDto,
    JoinServerByInviteResultDto, LoginResultDto,
};
use rim_services::auth::service::AuthService;
use rim_services::{ServiceError, ServiceResult};

use rim_services::events::Observers;

use crate::AuthRepos;
use crate::repos::AppRepos;
use crate::util::ensure_server_owner;

#[derive(Clone)]
pub struct DefaultAuthService {
    repos: AuthRepos,
    db: Option<DatabaseConnection>,
    bootstrap_secret: Option<String>,
    session_ttl: Duration,
    observers: Observers,
}

impl DefaultAuthService {
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

    fn new_csrf_token() -> String {
        // Reuse session token generator for entropy; different prefix.
        let t = generate_session_token();
        t.strip_prefix("rim_sess_")
            .map(|s| format!("rim_csrf_{s}"))
            .unwrap_or_else(|| format!("rim_csrf_{t}"))
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
            .await
            .map_err(rim_services::map_repo_error)?;

        let csrf = Self::new_csrf_token();
        Ok((sess, raw_token, csrf))
    }

    async fn ensure_no_users_exist(&self) -> ServiceResult<()> {
        let exists = self
            .repos
            .user_repo
            .any_users_exist()
            .await
            .map_err(rim_services::map_repo_error)?;
        if exists {
            return Err(ServiceError::PermissionDenied);
        }
        Ok(())
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
                rim_services::map_repo_error(e)
            })?;

        info!(username = %username, "add server");
        let server = repos
            .server_repo
            .create_server(server_name)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, kind = ?e.kind(), "create_server failed");
                rim_services::map_repo_error(e)
            })?;

        info!(username = %username, "add member");
        let _member = repos
            .server_member_repo
            .add_member(server.id, user.id, ServerMemberRole::Owner)
            .await
            .map_err(rim_services::map_repo_error)?;

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
            .await
            .map_err(rim_services::map_repo_error)?;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        let csrf = Self::new_csrf_token();

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
            .await
            .map_err(rim_services::map_repo_error)?
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
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if sess.user_id != actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        self.repos
            .session_repo
            .revoke_session(session_id)
            .await
            .map_err(rim_services::map_repo_error)?;

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
            .await
            .map_err(rim_services::map_repo_error)?;

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
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if inv.revoked_at.is_some() {
            return Err(ServiceError::PermissionDenied);
        }
        if let Some(expires_at) = inv.expires_at
            && expires_at < Utc::now()
        {
            return Err(ServiceError::PermissionDenied);
        }

        // Ensure username not taken.
        if self
            .repos
            .user_repo
            .find_by_username(username)
            .await
            .map_err(rim_services::map_repo_error)?
            .is_some()
        {
            return Err(ServiceError::Conflict(
                "username already exists".to_string(),
            ));
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

        let user = repos
            .user_repo
            .create_user(rim_core::user::model::NewUser {
                username: username.to_string(),
                password: password_hash,
            })
            .await
            .map_err(rim_services::map_repo_error)?;

        let _member = repos
            .server_member_repo
            .add_member(inv.server_id, user.id, ServerMemberRole::Guest)
            .await
            .map_err(rim_services::map_repo_error)?;

        let consumed = repos
            .invite_repo
            .try_consume(inv.id)
            .await
            .map_err(rim_services::map_repo_error)?;
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
            .await
            .map_err(rim_services::map_repo_error)?;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        self.observers
            .workspace
            .on_member_joined(inv.server_id, user.id, username);

        let csrf = Self::new_csrf_token();

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
            .await
            .map_err(rim_services::map_repo_error)?
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
            .await
            .map_err(rim_services::map_repo_error)?;
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

        let consumed = repos
            .invite_repo
            .try_consume(inv.id)
            .await
            .map_err(rim_services::map_repo_error)?;
        if !consumed {
            return Err(ServiceError::PermissionDenied);
        }

        let _member = repos
            .server_member_repo
            .add_member(inv.server_id, actor.user_id, ServerMemberRole::Guest)
            .await
            .map_err(rim_services::map_repo_error)?;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        // Emit MemberJoined event so other workspace members see the new member in real-time.
        if let Ok(Some(user)) = self
            .repos
            .user_repo
            .get_by_id(actor.user_id)
            .await
            .map_err(rim_services::map_repo_error)
        {
            self.observers
                .workspace
                .on_member_joined(inv.server_id, actor.user_id, &user.username);
        }

        info!(user_id = %actor.user_id, server_id = %inv.server_id, "user joined server via invite");

        Ok(JoinServerByInviteResultDto {
            server_id: inv.server_id,
        })
    }

    #[instrument(name = "auth.list_invites", skip_all, fields(actor_id = %query.actor.user_id, server_id = %query.server_id))]
    async fn list_invites(
        &self,
        query: ListInvitesQuery,
    ) -> ServiceResult<Vec<InviteDto>> {
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
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(invites.into_iter().map(Self::to_invite_dto).collect())
    }

    #[instrument(name = "auth.revoke_invite", skip_all, fields(actor_id = %cmd.actor.user_id, invite_id = %cmd.invite_id))]
    async fn revoke_invite(&self, cmd: RevokeInviteCmd) -> ServiceResult<()> {
        let inv = self
            .repos
            .invite_repo
            .get_by_id(cmd.invite_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        ensure_server_owner(
            self.repos.server_member_repo.as_ref(),
            inv.server_id,
            cmd.actor.user_id,
        )
        .await?;

        self.repos
            .invite_repo
            .revoke_invite(cmd.invite_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(invite_id = %cmd.invite_id, "invite revoked");
        Ok(())
    }
}
