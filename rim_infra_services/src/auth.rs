use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use tracing::{info, instrument};

use rim_auth::password::{hash_password, verify_password};
use rim_auth::session_token::{generate_session_token, hash_token};
use rim_core::auth::model::{NewInvite, NewSession};
use rim_core::server_member::model::ServerMemberRole;
use rim_services::auth::dto::{
    AcceptInviteResultDto, AuthSessionDto, BootstrapResultDto, CreateInviteResultDto, InviteDto,
    LoginResultDto,
};
use rim_services::auth::service::AuthService;
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::AuthRepos;
use crate::util::ensure_server_owner;

#[derive(Clone)]
pub struct DefaultAuthService {
    repos: AuthRepos,
    bootstrap_secret: Option<String>,
    session_ttl: Duration,
}

impl DefaultAuthService {
    pub fn new(repos: AuthRepos, bootstrap_secret: Option<String>, session_ttl: Duration) -> Self {
        Self {
            repos,
            bootstrap_secret,
            session_ttl,
        }
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
    #[instrument(name = "auth.bootstrap", skip_all, fields(username = %username))]
    async fn bootstrap(
        &self,
        secret: &str,
        username: &str,
        password: &str,
        server_name: &str,
    ) -> ServiceResult<BootstrapResultDto> {
        info!(username = %username, "check bootstrap secret");
        let expected = self
            .bootstrap_secret
            .as_deref()
            .ok_or(ServiceError::PermissionDenied)?;
        if secret != expected {
            return Err(ServiceError::PermissionDenied);
        }

        self.ensure_no_users_exist().await?;

        info!(username = %username, "hash the password");
        let password_hash = hash_password(password).map_err(ServiceError::internal)?;

        info!(username = %username, "add user");
        let user = self
            .repos
            .user_repo
            .create_user(rim_core::user::model::NewUser {
                username: username.to_string(),
                password: password_hash,
            })
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(username = %username, "add server");
        let server = self
            .repos
            .server_repo
            .create_server(server_name)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(username = %username, "add member");
        let _member = self
            .repos
            .server_member_repo
            .add_member(server.id, user.id, ServerMemberRole::Owner)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(username = %username, "create session");
        let (sess, raw_token, csrf) = self.create_session_for_user(user.id, None, None).await?;

        info!(user_id = %user.id, server_id = %server.id, "bootstrapped instance");

        Ok(BootstrapResultDto {
            user_id: user.id,
            server_id: server.id,
            session: Self::to_session_dto(&sess),
            session_token: raw_token,
            csrf_token: csrf,
        })
    }

    #[instrument(name = "auth.login", skip_all, fields(username = %username))]
    async fn login(&self, username: &str, password: &str) -> ServiceResult<LoginResultDto> {
        let user = self
            .repos
            .user_repo
            .find_by_username(username)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::PermissionDenied)?;

        let ok = verify_password(password, &user.password_hash).map_err(ServiceError::internal)?;
        if !ok {
            return Err(ServiceError::PermissionDenied);
        }

        let (sess, raw_token, csrf) = self.create_session_for_user(user.id, None, None).await?;

        Ok(LoginResultDto {
            user_id: user.id,
            session: Self::to_session_dto(&sess),
            session_token: raw_token,
            csrf_token: csrf,
        })
    }

    #[instrument(name = "auth.logout", skip_all, fields(actor_id = %actor.user_id, session_id = %session_id))]
    async fn logout(
        &self,
        actor: Actor,
        session_id: rim_core::auth::model::SessionId,
    ) -> ServiceResult<()> {
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

    #[instrument(name = "auth.create_invite", skip_all, fields(actor_id = %actor.user_id, server_id = %server_id, max_uses))]
    async fn create_invite(
        &self,
        actor: Actor,
        server_id: rim_core::server::model::ServerId,
        expires_at: Option<DateTime<Utc>>,
        max_uses: i32,
    ) -> ServiceResult<CreateInviteResultDto> {
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

    #[instrument(name = "auth.accept_invite", skip_all, fields(username = %username))]
    async fn accept_invite(
        &self,
        invite_token: &str,
        username: &str,
        password: &str,
    ) -> ServiceResult<AcceptInviteResultDto> {
        if username.trim().is_empty() {
            return Err(ServiceError::Validation("username required".to_string()));
        }
        if password.is_empty() {
            return Err(ServiceError::Validation("password required".to_string()));
        }

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
        if let Some(expires_at) = inv.expires_at {
            if expires_at < Utc::now() {
                return Err(ServiceError::PermissionDenied);
            }
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
        let user = self
            .repos
            .user_repo
            .create_user(rim_core::user::model::NewUser {
                username: username.to_string(),
                password: password_hash,
            })
            .await
            .map_err(rim_services::map_repo_error)?;

        let _member = self
            .repos
            .server_member_repo
            .add_member(inv.server_id, user.id, ServerMemberRole::Guest)
            .await
            .map_err(rim_services::map_repo_error)?;

        let consumed = self
            .repos
            .invite_repo
            .try_consume(inv.id)
            .await
            .map_err(rim_services::map_repo_error)?;
        if !consumed {
            // Don't leak whether it's exhausted vs missing.
            return Err(ServiceError::PermissionDenied);
        }

        let (sess, raw_token, csrf) = self.create_session_for_user(user.id, None, None).await?;

        Ok(AcceptInviteResultDto {
            user_id: user.id,
            server_id: inv.server_id,
            session: Self::to_session_dto(&sess),
            session_token: raw_token,
            csrf_token: csrf,
        })
    }
}
