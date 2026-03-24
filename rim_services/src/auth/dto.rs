use chrono::{DateTime, Utc};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AuthSessionDto {
    pub user_id: UserId,
    pub session_id: rim_core::auth::model::SessionId,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InviteDto {
    pub id: rim_core::auth::model::InviteId,
    pub server_id: ServerId,
    pub created_by_user_id: UserId,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: i32,
    pub use_count: i32,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BootstrapResultDto {
    pub user_id: UserId,
    pub server_id: ServerId,
    pub session: AuthSessionDto,
    pub session_token: String,
    pub csrf_token: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LoginResultDto {
    pub user_id: UserId,
    pub session: AuthSessionDto,
    pub session_token: String,
    pub csrf_token: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CreateInviteResultDto {
    pub invite: InviteDto,
    pub invite_token: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AcceptInviteResultDto {
    pub user_id: UserId,
    pub server_id: ServerId,
    pub session: AuthSessionDto,
    pub session_token: String,
    pub csrf_token: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct JoinServerByInviteResultDto {
    pub server_id: ServerId,
}

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RequestAuthResultDto {
    pub user_id: Option<UserId>,
    pub session_id: Option<rim_core::auth::model::SessionId>,
}
