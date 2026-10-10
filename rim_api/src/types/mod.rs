//! GraphQL object, input and enum types, split per domain to mirror the
//! resolver modules in `crate::query` and `crate::mutation`.
//!
//! Every type is re-exported here, so `crate::types::X` (and
//! `rim_api::types::X`) keeps working regardless of which file defines it.
//! Cursor inputs/outputs and the generic page types live in `pagination`.

pub(crate) mod attachment;
pub(crate) mod audit;
pub(crate) mod channel_lens;
pub(crate) mod command;
pub(crate) mod command_result;
pub(crate) mod conversation;
pub(crate) mod custom_emoji;
pub(crate) mod device_link;
pub(crate) mod e2ee;
pub(crate) mod invite;
pub(crate) mod key_identity;
pub(crate) mod keybind;
pub(crate) mod macro_def;
pub(crate) mod message;
pub(crate) mod moderation;
pub(crate) mod notification;
pub(crate) mod pagination;
pub(crate) mod pipeline;
pub(crate) mod relay;
pub(crate) mod remote_workspace;
pub(crate) mod server_info;
pub(crate) mod thread;
pub(crate) mod user;
pub(crate) mod workspace;
pub(crate) mod workspace_plugin;
pub(crate) mod workspace_state;

pub use attachment::{Attachment, AttachmentStatusEnum, UploadTicket};
pub use audit::{
    AuditLogEntry, AuditLogFilterInput, AuditLogPage, AuditLogPageInfo, AuditLogPaginationInput,
    AuditOutcomeGql, TypedVerbGql,
};
pub use channel_lens::{
    ChannelLens, FilterConditionGql, FilterConditionInput, FilterField, FilterLogic, FilterOp,
    FilterSpecGql, FilterSpecInput,
};
pub use command::{
    Command, CommandArgument, CommandArgumentType, CommandContextInput, CommandExecutionTarget,
    CommandInitiator, CommandSensitivity, CommandSourceKind, CommandSurface,
};
pub use command_result::{
    CommandResult, CommandResultStatus, ConfirmationChallenge, ConfirmationProofInput,
    InBandTokenInput, PipeStageResult,
};
pub use conversation::{
    ChannelListFilterInput, Conversation, ConversationTypeEnum, MutedConversation, UnreadCount,
};
pub use custom_emoji::CustomEmoji;
pub use device_link::{
    DeviceLink, DeviceLinkState, SealedMessage, SealedMessageInput, StartedDeviceLink,
};
pub use e2ee::{
    ClaimMlsKeyPackagesInput, ClaimedKeyPackage, EditEncryptedMessageInput, MessageEncryption,
    MlsCommit, MlsCommitPage, MlsDevice, MlsDeviceRefInput, MlsGroup, MlsKeyPackageStatus,
    MlsPendingDevice, MlsReaddRequest, MlsWelcome, PrincipalDevice, PrincipalDevices,
    ReportEncryptedMessageInput, RequestMlsReaddInput, SendEncryptedMessageInput,
    SubmitMlsCommitInput,
};
pub use invite::{
    InviteTokenPreviewGql, InviteTokenTypeGql, RegistrationModeGql, SignupWithInvitePayloadGql,
    WorkspaceInvitePreviewGql,
};
pub use key_identity::{
    DeviceKey, EnrollIdentityKeyInput, KeyIdentity, KeyLogEntry, KeyLogEntryKind,
    KeyLoginChallenge, ProfileCard, RotateIdentityKeyInput, SignedStatement, SignedStatementInput,
    SignupWithKeyInput,
};
pub use keybind::{Keybind, KeybindContext, KeybindScope, KeybindSource};
pub use macro_def::{MacroDef, MacroStep};
pub use message::{
    ConversationChangesStatus, ConversationMessageChanges, LinkPreview, Message, MessageChange,
    MessageChangeCursorInput, MessageChangeKind, MessageChangesInput, ReactionGroup,
    SearchMessagesInput, SearchResultMessage,
};
pub use moderation::{
    MemberTimeout, MessageReport, MessageReportCursorInput, MessageReportCursorOutput,
    MessageReportPage, MessageReportStatus, ReportOutcome, ServerBan,
};
pub use notification::{
    ConversationNotificationLevel, DigestFrequency, DndWindow, DndWindowInput, Notification,
    NotificationCursorInput, NotificationCursorOutput, NotificationLevel, NotificationPage,
    NotificationReason, NotificationSettings, UpdateNotificationSettingsInput,
};
pub use pagination::{
    ChannelCursorInput, ChannelCursorOutput, ChannelPage, ConversationPage, InboxCursorInput,
    InboxCursorOutput, MessageCursorInput, MessageCursorOutput, MessagePage, PageInfo,
    SearchResultPage,
};
pub use pipeline::{
    CreatePipelineInput, PipelineActionGql, PipelineActionInput, PipelineFilterConditionGql,
    PipelineFilterConditionInput, PipelineFilterGql, PipelineFilterInput, PipelineGql,
    PipelineTriggerGql, PipelineTriggerInput, PipelineTriggerKindGql, UpdatePipelineInput,
};
pub use relay::{
    Contact, ConversationPredecessor, DmRequest, DmRequestStatus, PredecessorInput, RelayInfo,
    RelayItem, RelayItemPage, RemoteIdentity, RemoteMemberInput, RemotePrincipalDevice,
    RemoteServer, StreamHead,
};
pub use remote_workspace::{
    JoinRemoteWorkspaceInput, RemoteJoinChallenge, RemoteJoinResult, RemoteSubscription,
    RevocationFanout, RevocationFanoutStatus,
};
pub use server_info::{API_VERSION, ServerFeatureInfo, ServerFeatureState, ServerInfo};
pub use thread::Thread;
pub use user::{PresenceStatus, User, UserPresenceGql};
pub use workspace::{Server, ServerMember, ServerMemberRoleGql};
pub use workspace_plugin::{
    AddWorkspacePluginInput, PluginRuntimeKind, PluginSignatureStatus, PluginSource,
    PluginSourceKind, UpdateWorkspacePluginInput, WorkspacePlugin, WorkspacePluginStatus,
};
pub use workspace_state::{
    DeleteWorkspaceStateInput, SetWorkspaceStateInput, WorkspaceStateEntryGql,
    WorkspaceStateScopeGql,
};
