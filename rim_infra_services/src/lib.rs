pub(crate) mod account_cap;
/// Public for `rim_server`, whose composition root runs the hourly purge and whose
/// `account purge` command runs it once.
pub mod account_deletion;
pub(crate) mod account_password;
pub(crate) mod audit;
#[cfg(test)]
mod audit_capture;
/// Public for `rim_server`, whose composition root starts the daily purge and whose
/// `audit purge` command runs it once.
pub mod audit_retention;
/// Public for `rim_server`, whose avatar upload route has every picture checked with it.
pub mod avatar_check;
/// Public for `rim_server`, whose avatar upload route re-encodes every picture with it.
pub mod avatar_image;
// Public: DefaultAuthService is re-exported below for integration tests
// (signup_txn_regression) that construct the service directly against a live database.
pub mod auth;
pub mod capability;
pub(crate) mod channel_lens;
pub(crate) mod clock;
pub mod command;
pub(crate) mod conversation;
pub(crate) mod custom_emoji;
/// Public for `rim_server`, whose composition root checks the configured default workspace at
/// start.
pub mod default_workspace;
pub(crate) mod default_workspace_rules;
pub(crate) mod device_link;
pub(crate) mod display_name;
pub(crate) mod dm_policy;
pub(crate) mod e2ee;
pub(crate) mod probation;
#[cfg(test)]
mod probation_fixtures;
pub(crate) mod request_gate;
// Public: the composition root (rim_server/src/app.rs) constructs
// EmailSender/SmtpEmailSender/LogEmailSender/SmtpConfig directly.
pub mod email;
/// Public for `rim_server`, whose composition root builds the instance gate from its
/// configuration, and for `rim_api`'s tests.
pub mod feature_gate;
pub mod file_storage;
/// Public for `rim_server`, which loads the key file at boot and runs `franking-key rotate`.
pub mod franking_keys;
// pub: the composition root (rim_server/src/app.rs) builds DefaultHostService and the outbox
// writer and sender tasks.
pub mod host;
/// Public for `rim_server`, whose `invite` command issues, lists and revokes the operator's
/// instance invites.
pub mod instance_invite;
/// Public for `rim_server`, whose `ip` command manages the deny list and whose edge refreshes
/// it.
pub mod ip_block;
pub(crate) mod key_auth;
pub(crate) mod keybind;
/// Public for `rim_server`, whose S2S routes rate-limit each peer with it.
pub mod keyed_rate_limiter;
// pub: rim_server loads the key file at boot and hands the key to the provider.
pub mod link_key;
pub(crate) mod link_preview;
pub mod macro_def;
pub(crate) mod mention;
pub(crate) mod message;
pub(crate) mod message_attachment;
pub(crate) mod message_reaction;
// Public: `rim_server` describes these metrics and samples the relay queue.
pub mod metrics;
pub(crate) mod moderation;
// Public: the notification digest/retention jobs are constructed directly
// by the composition root (rim_server/src/app.rs).
pub mod notification;
pub(crate) mod notification_preference;
/// Public for `rim_server`, whose `operator` command grants and revokes the role.
pub mod operator;
pub(crate) mod outbound;
// pub: rim_server loads the key file at boot and hands the key to the provider.
pub mod password_key;
pub(crate) mod pin;
pub(crate) mod pipeline;
pub(crate) mod plugin_bundle;
pub(crate) mod plugin_bundle_fetch;
pub mod presence;
pub(crate) mod provider;
pub(crate) mod public_join_link;
pub mod quota;
pub(crate) mod read_state;
pub mod registration_rate_limiter;
// pub: the composition root (rim_server/src/app.rs) builds DefaultRelayService.
pub mod relay;
pub mod remote_workspace;
pub(crate) mod repos;
pub(crate) mod request_auth;
/// Public for `rim_server`, whose configuration parses `RIM_RESERVED_NAMES` into it.
pub mod reserved_names;
pub mod s2s;
/// Public for `rim_server`, whose composition root runs the stale-attachment cleanup and the
/// workspace-state expiry purge.
pub mod scheduled_purges;
pub(crate) mod secret_file;
pub(crate) mod server;
pub(crate) mod server_member;
// pub: rim_server builds the registry, watches connections with it and runs the recheck.
pub(crate) mod session_key;
pub mod session_revocation;
pub(crate) mod signup;
pub mod storage;
/// Public for `rim_server`, whose composition root builds the text art check's client.
pub mod text_art_check;
/// Public for `rim_server`, whose composition root runs the text art check on message events.
pub mod text_art_moderation;
pub(crate) mod thread;
/// Public for `rim_server`, whose per-address, per-session and WebSocket limits use it.
pub mod token_bucket;
pub mod typing;
/// Public for `rim_server`, whose `principal clear-profile` command builds the user service.
pub mod user;
pub(crate) mod workspace_plugin;
pub(crate) mod workspace_state;

pub(crate) mod url_encode;
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
mod public_join_link_unit_tests;

#[cfg(test)]
mod account_password_fakes;

#[cfg(test)]
mod account_password_unit_tests;

#[cfg(test)]
mod conversation_unit_tests;

#[cfg(test)]
mod message_unit_tests;

#[cfg(test)]
mod moderation_unit_tests;

#[cfg(test)]
mod notification_unit_tests;

#[cfg(test)]
mod server_unit_tests;

#[cfg(test)]
mod server_member_unit_tests;

#[cfg(test)]
mod user_unit_tests;

#[cfg(test)]
mod key_auth_unit_tests;

#[cfg(test)]
mod command_tests;

#[cfg(test)]
mod device_link_unit_tests;

#[cfg(test)]
mod workspace_plugin_unit_tests;

#[cfg(test)]
mod plugin_bundle_unit_tests;

#[cfg(test)]
mod e2ee_test_support;

#[cfg(test)]
mod e2ee_unit_tests;

#[cfg(test)]
mod relay_unit_tests;

#[cfg(test)]
mod encrypted_message_unit_tests;

pub use auth::DefaultAuthService;
pub use keyed_rate_limiter::KeyedRateLimiter;
pub use provider::{DefaultAppServiceProvider, FederationHooks};
pub use registration_rate_limiter::RegistrationRateLimiter;
// pub: rim_server builds `AppRepos` at boot; the integration tests build both directly.
pub use repos::{AppRepos, CommandRepos};
pub(crate) use repos::{
    AttachmentRepos, AuthRepos, ConversationRepos, MessageRepos, PinRepos, ReactionRepos,
    ReadStateRepos, RequestAuthRepos, ServerMemberRepos, ServerRepos, ThreadRepos, UserRepos,
};
// Public for rim_server database metrics sampling.
pub mod usage_metrics;
