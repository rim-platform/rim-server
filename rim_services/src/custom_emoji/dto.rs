use chrono::{DateTime, Utc};
use rim_core::custom_emoji::model::{CustomEmoji, CustomEmojiId};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CustomEmojiDto {
    pub id: CustomEmojiId,
    pub server_id: ServerId,
    pub name: String,
    pub image_url: String,
    pub uploaded_by: UserId,
    pub created_at: DateTime<Utc>,
}

impl From<CustomEmoji> for CustomEmojiDto {
    fn from(e: CustomEmoji) -> Self {
        Self {
            id: e.id,
            server_id: e.server_id,
            name: e.name,
            image_url: e.image_url,
            uploaded_by: e.uploaded_by,
            created_at: e.created_at,
        }
    }
}
