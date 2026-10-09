use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
pub struct ServerMemberDto {
    pub server_id: ServerId,
    pub user_id: UserId,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub status_text: Option<String>,
    pub bio: Option<String>,
    pub member_role: ServerMemberRole,
    pub joined_at: chrono::DateTime<chrono::Utc>,
    /// End of the member's timeout while one is in force (Track 1.2), else `None`.
    pub timeout_until: Option<chrono::DateTime<chrono::Utc>>,
    /// The member's key identity, once enrolled; `None` for a password-only account.
    pub principal_id: Option<String>,
}
