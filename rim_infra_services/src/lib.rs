pub mod alert_rate_limiter;
pub(crate) mod audit;
// pub(crate) promoted to pub: DefaultAuthService is re-exported below for integration tests
// (signup_txn_regression) that construct the service directly against a live database.
pub mod auth;
pub mod capability;
pub(crate) mod channel_lens;
pub mod command;
pub(crate) mod conversation;
pub(crate) mod custom_emoji;
// pub(crate) promoted to pub: EmailSender/SmtpEmailSender/LogEmailSender/SmtpConfig are
// constructed directly by the composition root (rim_server/src/app.rs) — mirrors the
// DefaultAuthService precedent above (Track B3, 2026-07-10).
pub mod email;
// pub(crate) promoted to pub: EscalationScheduler impl + driver task are constructed directly
// by the composition root (rim_server/src/app.rs), mirroring the DefaultOnCallService precedent
// above.
pub mod escalation;
pub mod file_storage;
pub(crate) mod incident;
pub(crate) mod ingest_token;
pub(crate) mod key_auth;
pub(crate) mod keybind;
pub(crate) mod link_preview;
pub(crate) mod login_sources;
pub mod macro_def;
pub(crate) mod mention;
pub(crate) mod message;
pub(crate) mod message_attachment;
pub(crate) mod message_reaction;
pub(crate) mod moderation;
// pub(crate) promoted to pub: the notification digest/retention jobs are constructed directly
// by the composition root (rim_server/src/app.rs), mirroring the escalation driver above.
pub mod notification;
pub(crate) mod notification_preference;
// pub(crate) promoted to pub: DefaultOnCallService is constructed directly by the
// who_is_on_call_matches_pure_resolver integration test (rim_infra_services/tests/oncall_db_test.rs)
// against a live database — mirrors the DefaultAuthService precedent above.
pub mod oncall;
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
pub(crate) mod skill_profile;
pub mod storage;
pub(crate) mod thread;
pub mod typing;
pub(crate) mod user;
pub(crate) mod workspace_feature;
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
mod incident_unit_tests;

#[cfg(test)]
mod oncall_unit_tests;

#[cfg(test)]
mod key_auth_unit_tests;

#[cfg(test)]
mod escalation_policy_unit_tests;

#[cfg(test)]
mod command_tests;

pub use alert_rate_limiter::AlertRateLimiter;
pub use auth::DefaultAuthService;
pub use channel_lens::ChannelLensRepos;
pub use custom_emoji::CustomEmojiRepos;
pub use escalation::{EscalationPolicyRepos, InMemoryEscalationScheduler};
pub use incident::IncidentRepos;
pub use macro_def::MacroRepos;
pub use notification::NotificationRepos;
pub use notification_preference::NotificationPreferenceRepos;
pub use oncall::OnCallRepos;
pub use pipeline::PipelineRepos;
pub use provider::DefaultAppServiceProvider;
pub use registration_rate_limiter::RegistrationRateLimiter;
pub use repos::{
    AppRepos, AttachmentRepos, AuthRepos, CommandRepos, ConversationRepos, KeybindRepos,
    MentionRepos, MessageRepos, PinRepos, ReactionRepos, ReadStateRepos, RequestAuthRepos,
    ServerMemberRepos, ServerRepos, ThreadRepos, UserRepos,
};
pub use workspace_feature::WorkspaceFeatureRepos;
pub use workspace_state::WorkspaceStateRepos;
