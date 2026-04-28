pub(crate) mod action;
pub(crate) mod audit;
pub(crate) mod auth;
pub mod capability;
pub(crate) mod channel_lens;
pub mod command;
pub(crate) mod conversation;
pub(crate) mod custom_emoji;
pub(crate) mod email;
pub(crate) mod event;
pub mod file_storage;
pub(crate) mod incident;
pub(crate) mod integration;
pub(crate) mod keybind;
pub(crate) mod link_preview;
pub mod macro_def;
pub(crate) mod mention;
pub(crate) mod message;
pub(crate) mod message_attachment;
pub(crate) mod message_reaction;
pub(crate) mod notification_preference;
pub(crate) mod password_reset;
pub(crate) mod pin;
pub(crate) mod pipeline;
pub mod presence;
pub(crate) mod provider;
pub(crate) mod read_state;
pub mod registration_rate_limiter;
pub(crate) mod repos;
pub(crate) mod request_auth;
pub(crate) mod server;
pub(crate) mod server_member;
pub(crate) mod setting;
pub(crate) mod skill_profile;
pub mod storage;
pub(crate) mod thread;
pub mod typing;
pub(crate) mod user;
pub(crate) mod workspace_state;

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

#[cfg(test)]
mod incident_unit_tests;

#[cfg(test)]
mod command_tests;

pub use channel_lens::ChannelLensRepos;
pub use custom_emoji::CustomEmojiRepos;
pub use incident::IncidentRepos;
pub use macro_def::MacroRepos;
pub use notification_preference::NotificationPreferenceRepos;
pub use pipeline::PipelineRepos;
pub use provider::DefaultAppServiceProvider;
pub use registration_rate_limiter::{
    PreviewInviteRateLimiter, RegistrationRateLimiter, SignupRateLimiter,
};
pub use repos::{
    AppRepos, AttachmentRepos, AuthRepos, CommandRepos, ConversationRepos, KeybindRepos,
    MentionRepos, MessageRepos, PinRepos, ReactionRepos, ReadStateRepos, RequestAuthRepos,
    ServerMemberRepos, ServerRepos, ThreadRepos, UserRepos,
};
pub use workspace_state::WorkspaceStateRepos;
