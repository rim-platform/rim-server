use crate::custom_emoji;
use rim_core::custom_emoji::model::CustomEmoji;

impl From<custom_emoji::Model> for CustomEmoji {
    fn from(m: custom_emoji::Model) -> Self {
        Self {
            id: m.id.into(),
            server_id: m.server_id.into(),
            name: m.name,
            image_url: m.image_url,
            uploaded_by: m.uploaded_by.into(),
            created_at: m.created_at.into(),
        }
    }
}
