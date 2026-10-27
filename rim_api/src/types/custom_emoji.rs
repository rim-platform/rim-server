use async_graphql::SimpleObject;

// ── Custom Emoji ──────────────────────────────────────────────────────────

#[derive(SimpleObject, Clone, Debug)]
pub struct CustomEmoji {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub name: String,
    /// Where a member of the workspace loads the image: `/api/emoji/<id>` on this server, with the session cookie.
    pub image_url: String,
    pub uploaded_by: async_graphql::ID,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::custom_emoji::dto::CustomEmojiDto> for CustomEmoji {
    fn from(value: rim_services::custom_emoji::dto::CustomEmojiDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            name: value.name,
            image_url: value.image_url,
            uploaded_by: async_graphql::ID(value.uploaded_by.to_string()),
            created_at: value.created_at,
        }
    }
}
