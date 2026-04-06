pub mod action;
pub mod auth;
pub mod capability;
pub mod command;
pub mod conversation;
pub mod custom_emoji;
pub mod email;
pub mod event;
pub mod file_storage;
pub mod integration;
pub mod keybind;
pub mod link_preview;
pub mod macro_def;
pub mod mention;
pub mod message;
pub mod message_attachment;
pub mod message_reaction;
pub mod notification_preference;
pub mod password_reset;
pub mod pin;
pub mod presence;
pub mod provider;
pub mod read_state;
pub mod repos;
pub mod request_auth;
pub mod server;
pub mod server_member;
pub mod setting;
pub mod storage;
pub mod thread;
pub mod typing;
pub mod user;

pub(crate) mod util;

#[cfg(test)]
mod thread_unit_tests;

#[cfg(test)]
mod message_attachment_unit_tests;

#[cfg(test)]
mod message_reaction_unit_tests;

#[cfg(test)]
mod auth_unit_tests;

#[cfg(test)]
mod conversation_unit_tests;

#[cfg(test)]
mod message_unit_tests;

#[cfg(test)]
mod server_unit_tests;

#[cfg(test)]
mod server_member_unit_tests;

#[cfg(test)]
mod user_unit_tests;

pub use custom_emoji::CustomEmojiRepos;
pub use macro_def::MacroRepos;
pub use notification_preference::NotificationPreferenceRepos;
pub use provider::DefaultAppServiceProvider;
pub use repos::{
    AppRepos, AttachmentRepos, AuthRepos, CommandRepos, ConversationRepos, KeybindRepos,
    MentionRepos, MessageRepos, PinRepos, ReactionRepos, ReadStateRepos, RequestAuthRepos,
    ServerMemberRepos, ServerRepos, ThreadRepos, UserRepos,
};
