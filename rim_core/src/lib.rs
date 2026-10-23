pub mod account_deletion;
pub mod audit;
pub mod auth;
pub mod capability;
pub mod channel_lens;
pub mod command;
pub mod conversation;
pub mod conversation_member;
pub mod conversation_read_state;
pub mod custom_emoji;
// pub: rim_infra_db stores it, rim_infra_services claims and reads it.
pub mod default_workspace;
pub mod device_link;
// pub: rim_infra_db stores it, rim_services and rim_infra_services apply it, rim_api shows it.
pub mod dm_policy;
pub mod e2ee;
pub mod host;
pub mod ip_block;
pub mod key_identity;
pub mod keybind;
pub mod link_preview;
pub mod macro_def;
mod macros;
pub mod message;
pub mod message_attachment;
pub mod message_mention;
pub mod message_pin;
pub mod message_reaction;
pub mod message_thread;
pub mod moderation;
// Public for rim_infra_db aggregate repositories and rim_infra_services samplers.
pub mod monitoring;
pub mod notification;
pub mod notification_preference;
pub mod password_reset;
pub mod pipeline;
pub mod principal;
// pub: rim_infra_db stores it, rim_services and rim_infra_services apply it, rim_api shows it.
pub mod public_join_link;
pub mod relay;
pub mod repository;
pub mod s2s;
pub mod server;
pub mod server_feature;
pub mod server_member;
pub mod storage;
pub mod user;
pub mod util;
pub mod workspace_plugin;
pub mod workspace_state;

#[cfg(test)]
mod tests;
