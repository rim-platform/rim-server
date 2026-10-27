use crate::Actor;
use rim_core::custom_emoji::model::CustomEmojiId;
use rim_core::server::model::ServerId;

#[derive(Debug, Clone)]
pub struct CreateCustomEmojiCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub name: String,
    pub image_data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct DeleteCustomEmojiCmd {
    pub actor: Actor,
    pub emoji_id: CustomEmojiId,
}

#[derive(Debug, Clone)]
pub struct ListCustomEmojisQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

/// Read a custom emoji's stored image.
#[derive(Debug, Clone)]
pub struct CustomEmojiImageQuery {
    pub actor: Actor,
    pub emoji_id: CustomEmojiId,
}
