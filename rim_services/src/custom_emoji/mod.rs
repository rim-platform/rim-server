pub mod command;
pub mod dto;
pub mod service;

use rim_core::custom_emoji::model::CustomEmojiId;

/// Where a member of the emoji's workspace loads its image: a path on this server.
pub fn custom_emoji_image_path(emoji_id: CustomEmojiId) -> String {
    format!("/api/emoji/{emoji_id}")
}
