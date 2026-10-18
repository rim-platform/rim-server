//! GraphQL `Query` root, assembled from one `#[Object]` per domain via
//! async-graphql's `MergedObject`.
//!
//! The SDL lists merged fields in member order, so the member order of
//! [`Query`] fixes the field order of the schema (the frontend codegen diffs
//! against it). Where that order interleaves domains, a domain is split into
//! several objects (for example
//! `ConversationQueries`, `ReadStateQueries` and `ConversationMuteQueries`).
//! Do not reorder members; append new domains at the end, or next to the
//! object they extend.
//!
//! The domain objects are grouped five at a time (`QueryGroup1`, ...) only to
//! keep the nested `MergedObject<A, MergedObject<B, ...>>` type shallow. The
//! nesting depth is roughly (number of groups + members of the largest group);
//! one flat struct of ~30 members overflows rustc's default recursion limit
//! (128) when the schema is monomorphized, in `rim_api` and in every crate that
//! executes it (`rim_server`, integration tests). Six groups of five need
//! about 105. The grouping carries no meaning beyond field order: append a new
//! object to the last group, and start a new group once it holds five.

use async_graphql::MergedObject;

pub(crate) mod audit;
pub(crate) mod channel_lens;
pub(crate) mod command;
pub(crate) mod conversation;
pub(crate) mod custom_emoji;
pub(crate) mod device_link;
pub(crate) mod dm_policy;
pub(crate) mod e2ee;
pub(crate) mod instance_moderation;
pub(crate) mod invite;
pub(crate) mod key_identity;
pub(crate) mod keybind;
pub(crate) mod macro_def;
pub(crate) mod message;
pub(crate) mod moderation;
pub(crate) mod notification;
pub(crate) mod pipeline;
pub(crate) mod presence;
pub(crate) mod profile;
pub(crate) mod relay;
pub(crate) mod remote_workspace;
pub(crate) mod server_info;
pub(crate) mod thread;
pub(crate) mod workspace;
pub(crate) mod workspace_plugin;
pub(crate) mod workspace_state;

#[derive(MergedObject, Default)]
pub struct Query(
    QueryGroup1,
    QueryGroup2,
    QueryGroup3,
    QueryGroup4,
    QueryGroup5,
    QueryGroup6,
    QueryGroup7,
    QueryGroup8,
    QueryGroup9,
);

#[derive(MergedObject, Default)]
pub(crate) struct QueryGroup1(
    command::CommandQueries,
    keybind::KeybindQueries,
    conversation::ConversationQueries,
    message::MessageQueries,
);

#[derive(MergedObject, Default)]
pub(crate) struct QueryGroup2(
    thread::ThreadQueries,
    profile::ProfileQueries,
    workspace::WorkspaceQueries,
    conversation::ReadStateQueries,
);

#[derive(MergedObject, Default)]
pub(crate) struct QueryGroup3(
    workspace::WorkspaceMemberQueries,
    message::MessagePinQueries,
    presence::PresenceQueries,
);

#[derive(MergedObject, Default)]
pub(crate) struct QueryGroup4(
    conversation::ConversationMuteQueries,
    custom_emoji::CustomEmojiQueries,
    macro_def::MacroQueries,
    channel_lens::ChannelLensQueries,
);

#[derive(MergedObject, Default)]
pub(crate) struct QueryGroup5(
    workspace_state::WorkspaceStateQueries,
    pipeline::PipelineQueries,
);

#[derive(MergedObject, Default)]
pub(crate) struct QueryGroup6(
    audit::AuditLogQueries,
    invite::RegistrationQueries,
    moderation::ModerationQueries,
);

#[derive(MergedObject, Default)]
pub(crate) struct QueryGroup7(
    notification::NotificationQueries,
    key_identity::KeyIdentityQueries,
    device_link::DeviceLinkQueries,
    workspace_plugin::WorkspacePluginQueries,
    e2ee::E2eeQueries,
);

#[derive(MergedObject, Default)]
pub(crate) struct QueryGroup8(
    relay::RelayQueries,
    remote_workspace::RemoteWorkspaceQueries,
    server_info::ServerInfoQueries,
    instance_moderation::InstanceModerationQueries,
    dm_policy::DmPolicyQueries,
);

#[derive(MergedObject, Default)]
pub(crate) struct QueryGroup9(command::UserCommandQueries);
