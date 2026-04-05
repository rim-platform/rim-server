use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};

model_id!(CustomEmojiId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomEmoji {
    pub id: CustomEmojiId,
    pub server_id: ServerId,
    pub name: String,
    pub image_url: String,
    pub uploaded_by: UserId,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewCustomEmoji {
    pub server_id: ServerId,
    pub name: String,
    pub image_url: String,
    pub uploaded_by: UserId,
}
