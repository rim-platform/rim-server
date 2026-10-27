use chrono::{DateTime, Utc};
use rim_core::custom_emoji::model::{CustomEmoji, CustomEmojiId};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
pub struct CustomEmojiDto {
    pub id: CustomEmojiId,
    pub server_id: ServerId,
    pub name: String,
    /// The image's URL path, from [`super::custom_emoji_image_path`].
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
            image_url: super::custom_emoji_image_path(e.id),
            uploaded_by: e.uploaded_by,
            created_at: e.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dto_names_the_image_by_its_route() {
        let id = CustomEmojiId::new();
        let dto = CustomEmojiDto::from(CustomEmoji {
            id,
            server_id: ServerId::new(),
            name: "party".to_string(),
            image_url: "s/emoji/x.png".to_string(),
            uploaded_by: UserId::new(),
            created_at: Utc::now(),
        });
        assert_eq!(dto.image_url, format!("/api/emoji/{id}"));
    }
}
