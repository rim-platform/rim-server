use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ServerMemberDto {
    pub server_id: ServerId,
    pub user_id: UserId,
    pub username: String,
    pub member_role: String,
    pub joined_at: chrono::DateTime<chrono::Utc>,
}
