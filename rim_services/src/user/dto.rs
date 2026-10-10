use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
pub struct UserDto {
    pub id: UserId,
    pub username: String,

    // Optional, but common:
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub status_text: Option<String>,
    pub bio: Option<String>,
    /// Key-based principal id once the account enrolled a key.
    pub principal_id: Option<String>,
}
