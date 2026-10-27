//! GraphQL `Mutation` root, assembled from one `#[Object]` per domain via
//! async-graphql's `MergedObject`.
//!
//! The SDL lists merged fields in member order, so the member order of
//! [`Mutation`] fixes the field order of the schema (the frontend codegen diffs
//! against it). Where that order interleaves domains, a domain is split into
//! several objects (for example
//! `KeybindMutations` and `KeybindDeleteMutations`). Do not reorder members;
//! append new domains at the end, or next to the object they extend.
//!
//! The domain objects are grouped five at a time (`MutationGroup1`, ...) only to
//! keep the nested `MergedObject<A, MergedObject<B, ...>>` type shallow. The
//! nesting depth is roughly (number of groups + members of the largest group);
//! one flat struct of ~30 members overflows rustc's default recursion limit
//! (128) when the schema is monomorphized, in `rim_api` and in every crate that
//! executes it (`rim_server`, integration tests). Six groups of five need
//! about 105. The grouping carries no meaning beyond field order: append a new
//! object to the last group, and start a new group once it holds five.

use async_graphql::MergedObject;

pub(crate) mod account;
pub(crate) mod account_password;
pub(crate) mod attachment;
pub(crate) mod auth;
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
pub(crate) mod public_join_link;
pub(crate) mod relay;
pub(crate) mod remote_workspace;
pub(crate) mod session_key;
pub(crate) mod workspace;
pub(crate) mod workspace_plugin;
pub(crate) mod workspace_state;

#[derive(MergedObject, Default)]
pub struct Mutation(
    MutationGroup1,
    MutationGroup2,
    MutationGroup3,
    MutationGroup4,
    MutationGroup5,
    MutationGroup6,
    MutationGroup7,
    MutationGroup8,
    MutationGroup9,
    MutationGroup10,
);

#[derive(MergedObject, Default)]
pub(crate) struct MutationGroup1(
    auth::AuthMutations,
    invite::InviteMutations,
    message::MessageMutations,
    conversation::ConversationMutations,
    message::MessageEditMutations,
);

#[derive(MergedObject, Default)]
pub(crate) struct MutationGroup2(
    attachment::AttachmentMutations,
    command::CommandMutations,
    keybind::KeybindMutations,
    message::MessageReactionMutations,
    conversation::ReadStateMutations,
);

#[derive(MergedObject, Default)]
pub(crate) struct MutationGroup3(
    workspace::WorkspaceMutations,
    keybind::KeybindDeleteMutations,
    message::MessagePinMutations,
    presence::PresenceMutations,
    conversation::ConversationMuteMutations,
);

#[derive(MergedObject, Default)]
pub(crate) struct MutationGroup4(
    custom_emoji::CustomEmojiMutations,
    macro_def::MacroMutations,
    channel_lens::ChannelLensMutations,
);

#[derive(MergedObject, Default)]
pub(crate) struct MutationGroup5(workspace_state::WorkspaceStateMutations);

#[derive(MergedObject, Default)]
pub(crate) struct MutationGroup6(
    pipeline::PipelineMutations,
    profile::ProfileMutations,
    moderation::ModerationMutations,
    notification::NotificationMutations,
);

#[derive(MergedObject, Default)]
pub(crate) struct MutationGroup7(
    key_identity::KeyIdentityMutations,
    device_link::DeviceLinkMutations,
    workspace_plugin::WorkspacePluginMutations,
    e2ee::E2eeMutations,
    relay::RelayMutations,
);

#[derive(MergedObject, Default)]
pub(crate) struct MutationGroup8(
    remote_workspace::RemoteWorkspaceMutations,
    workspace::WorkspaceOwnerMutations,
    account::AccountMutations,
    instance_moderation::InstanceModerationMutations,
    conversation::ConversationInviteMutations,
);

#[derive(MergedObject, Default)]
pub(crate) struct MutationGroup9(
    dm_policy::DmPolicyMutations,
    invite::WorkspaceInviteMutations,
    public_join_link::PublicJoinLinkMutations,
    account_password::AccountPasswordMutations,
    session_key::SessionKeyMutations,
);

#[derive(MergedObject, Default)]
pub(crate) struct MutationGroup10(workspace::WorkspaceMembershipMutations);
