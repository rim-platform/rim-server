use chrono::{DateTime, Utc};

use rim_core::server::model::ServerId;

use crate::Actor;

#[derive(Debug, Clone)]
pub struct BootstrapCmd {
    pub secret: String,
    pub username: String,
    pub password: String,
    pub server_name: String,
}

#[derive(Debug, Clone)]
pub struct LoginCmd {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct LogoutCmd {
    pub actor: Actor,
    pub session_id: rim_core::auth::model::SessionId,
}

#[derive(Debug, Clone)]
pub struct CreateInviteCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: i32,
}

#[derive(Debug, Clone)]
pub struct AcceptInviteCmd {
    pub invite_token: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub struct JoinServerByInviteCmd {
    pub actor: Actor,
    pub invite_token: String,
}
