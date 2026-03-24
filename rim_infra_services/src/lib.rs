pub mod action;
pub mod auth;
pub mod capability;
pub mod command;
pub mod conversation;
pub mod event;
pub mod integration;
pub mod keybind;
pub mod message;
pub mod message_attachment;
pub mod message_reaction;
pub mod provider;
pub mod repos;
pub mod request_auth;
pub mod server;
pub mod server_member;
pub mod setting;
pub mod thread;
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

pub use provider::DefaultAppServiceProvider;
pub use repos::{
    AppRepos, AttachmentRepos, AuthRepos, CommandRepos, ConversationRepos, KeybindRepos,
    MessageRepos, ReactionRepos, RequestAuthRepos, ServerMemberRepos, ServerRepos, ThreadRepos,
    UserRepos,
};
