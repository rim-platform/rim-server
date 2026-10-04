use serde::{Deserialize, Serialize};

use rim_core::conversation::model::ConversationId;
use rim_core::escalation::model::EscalationPolicyId;
use rim_core::incident::model::IncidentId;
use rim_core::message::model::MessageId;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::pipeline::model::PipelineId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use crate::message::dto::MessageDto;
use crate::workspace_state::dto::WorkspaceStateScopeDto;

/// Rich event types with full data for real-time updates.
/// Clients receive these directly — no need to re-fetch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkspaceEvent {
    NewMessage {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
        message: MessageEventData,
    },
    MessageUpdated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
        message: MessageEventData,
    },
    MessageDeleted {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
        message_id: MessageId,
        thread_id: Option<MessageThreadId>,
    },
    ReactionsUpdated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
        message_id: MessageId,
        reactions: Vec<ReactionGroupData>,
    },
    ConversationCreated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        title: Option<String>,
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    MemberJoined {
        workspace_id: ServerId,
        user_id: UserId,
        username: String,
    },
    ThreadCreated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        thread_id: MessageThreadId,
        root_message_id: MessageId,
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    MemberAddedToConversation {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    MemberRemovedFromConversation {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    ConversationUpdated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
    },
    /// Carries `conversation_type` (like `TypingStarted`) so the subscription filter can treat
    /// channels as visible to all workspace members without relying on `member_user_ids` —
    /// channels have no `conversation_member` rows, so that list is always empty for a
    /// channel-scoped pin. `skip_serializing` keeps the client wire shape unchanged: this field
    /// is server-side-only, and the frontend pin handlers already shipped without it.
    MessagePinned {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        message_id: MessageId,
        pinned_by: UserId,
        #[serde(skip_serializing)]
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    /// See [`Self::MessagePinned`] for why `conversation_type` is carried (`skip_serializing`,
    /// server-side-only).
    MessageUnpinned {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        message_id: MessageId,
        #[serde(skip_serializing)]
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    TypingStarted {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
        #[serde(skip_serializing)]
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    ReadStateUpdated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
        last_read_at: chrono::DateTime<chrono::Utc>,
    },
    PresenceChanged {
        workspace_id: ServerId,
        user_id: UserId,
        status: String,
    },
    UserMentioned {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        message_id: MessageId,
        #[serde(skip_serializing)]
        mentioned_user_ids: Vec<UserId>,
        author_id: UserId,
    },
    LinkPreviewReady {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        message_id: MessageId,
    },
    WorkspaceStateChanged {
        workspace_id: ServerId,
        key: String,
        value: serde_json::Value,
        scope: WorkspaceStateScopeDto,
        user_id: Option<UserId>,
        version: i64,
    },
    WorkspaceStateDeleted {
        workspace_id: ServerId,
        key: String,
        scope: WorkspaceStateScopeDto,
        user_id: Option<UserId>,
    },
    PipelineFired {
        workspace_id: ServerId,
        pipeline_id: PipelineId,
        pipeline_name: String,
        trigger_event: String,
        actions_executed: usize,
    },
    PipelineSuspended {
        workspace_id: ServerId,
        pipeline_id: PipelineId,
        pipeline_name: String,
        reason: String,
    },
    /// A new incident was declared (manually via `start_incident`, or by alert ingestion —
    /// see `IncidentService::ingest_alert`).
    ///
    /// Carries `conversation_type` (like `NewMessage`/`ConversationCreated`) rather than
    /// relying on `member_user_ids` alone: incidents aren't restricted to channels the way
    /// alert-ingest tokens are (`start_incident` accepts any conversation type), and channel
    /// conversations have no `conversation_member` rows, so `member_user_ids` would be empty
    /// for a channel-scoped incident. The subscription filter treats `Channel` as visible to
    /// all workspace members and falls back to `member_user_ids` otherwise.
    IncidentDeclared {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        incident_id: IncidentId,
        severity: Option<String>,
        title: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    /// An incident's status changed (mitigated/resolved/reopened, or updated in place by a
    /// repeat alert with the same fingerprint). See [`Self::IncidentDeclared`] for why
    /// `conversation_type` is carried alongside `member_user_ids`.
    IncidentUpdated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        incident_id: IncidentId,
        status: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    /// An escalation step fired (mention + email best-effort actions performed) — see
    /// `IncidentService::fire_escalation_step` (C1b, §5/§9). Carries `conversation_type` for the
    /// same channel-visibility-bypass reason as `IncidentDeclared`.
    EscalationStepFired {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        incident_id: IncidentId,
        escalation_policy_id: EscalationPolicyId,
        step_index: i32,
        /// Serialized (not `skip_serializing`) — the FE shows who was paged.
        target_user_ids: Vec<UserId>,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    /// An incident was acknowledged — see `IncidentService::acknowledge_incident` (C1b, §4/§9).
    IncidentAcknowledged {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        incident_id: IncidentId,
        acknowledged_by: UserId,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
}

/// Event payload for message-related events.
///
/// **Frontend note:** all fields listed here are sent over the WebSocket transport.
/// Ensure the client maps every field — in particular `server_id`, `thread_id`,
/// `root_thread_id`, `has_attachments`, and `parent_message_id` which are easy to
/// overlook when converting to the client-side cache model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEventData {
    pub id: MessageId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    /// Thread this message belongs to, if it is a thread reply.
    pub thread_id: Option<rim_core::message_thread::model::MessageThreadId>,
    /// Thread rooted at this message, if one exists (only set for root messages).
    pub root_thread_id: Option<rim_core::message_thread::model::MessageThreadId>,
    pub author_id: UserId,
    pub text: Option<String>,
    pub has_attachments: bool,
    pub parent_message_id: Option<MessageId>,
    pub forwarded_from_message_id: Option<MessageId>,
    pub is_deleted: bool,
    pub sent_at: chrono::DateTime<chrono::Utc>,
    pub reactions: Vec<ReactionGroupData>,
}

impl From<&MessageDto> for MessageEventData {
    fn from(dto: &MessageDto) -> Self {
        Self {
            id: dto.id,
            server_id: dto.server_id,
            conversation_id: dto.conversation_id,
            thread_id: dto.thread_id,
            root_thread_id: dto.root_thread_id,
            author_id: dto.author_id,
            text: if dto.is_deleted {
                None
            } else {
                dto.text.clone()
            },
            has_attachments: dto.has_attachments,
            parent_message_id: dto.parent_message_id,
            forwarded_from_message_id: dto.forwarded_from_message_id,
            is_deleted: dto.is_deleted,
            sent_at: dto.sent_at,
            reactions: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionGroupData {
    pub emoji: String,
    pub count: u32,
    pub user_ids: Vec<UserId>,
}

impl WorkspaceEvent {
    /// Returns the workspace (server) ID as a string for subscription filtering.
    pub fn workspace_id(&self) -> String {
        match self {
            Self::NewMessage { workspace_id, .. }
            | Self::MessageUpdated { workspace_id, .. }
            | Self::MessageDeleted { workspace_id, .. }
            | Self::ReactionsUpdated { workspace_id, .. }
            | Self::ConversationCreated { workspace_id, .. }
            | Self::ConversationUpdated { workspace_id, .. }
            | Self::TypingStarted { workspace_id, .. }
            | Self::MessagePinned { workspace_id, .. }
            | Self::MessageUnpinned { workspace_id, .. }
            | Self::ThreadCreated { workspace_id, .. }
            | Self::MemberJoined { workspace_id, .. }
            | Self::MemberAddedToConversation { workspace_id, .. }
            | Self::MemberRemovedFromConversation { workspace_id, .. }
            | Self::ReadStateUpdated { workspace_id, .. }
            | Self::PresenceChanged { workspace_id, .. }
            | Self::UserMentioned { workspace_id, .. }
            | Self::LinkPreviewReady { workspace_id, .. }
            | Self::WorkspaceStateChanged { workspace_id, .. }
            | Self::WorkspaceStateDeleted { workspace_id, .. }
            | Self::PipelineFired { workspace_id, .. }
            | Self::PipelineSuspended { workspace_id, .. }
            | Self::IncidentDeclared { workspace_id, .. }
            | Self::IncidentUpdated { workspace_id, .. }
            | Self::EscalationStepFired { workspace_id, .. }
            | Self::IncidentAcknowledged { workspace_id, .. } => workspace_id.to_string(),
        }
    }

    /// Returns the member user IDs for visibility filtering, if applicable.
    pub fn member_user_ids(&self) -> Option<&[UserId]> {
        match self {
            Self::NewMessage {
                member_user_ids, ..
            }
            | Self::MessageUpdated {
                member_user_ids, ..
            }
            | Self::MessageDeleted {
                member_user_ids, ..
            }
            | Self::ReactionsUpdated {
                member_user_ids, ..
            }
            | Self::ConversationCreated {
                member_user_ids, ..
            }
            | Self::ThreadCreated {
                member_user_ids, ..
            } => Some(member_user_ids),
            Self::MemberAddedToConversation {
                member_user_ids, ..
            }
            | Self::MemberRemovedFromConversation {
                member_user_ids, ..
            } => Some(member_user_ids),
            Self::MemberJoined { .. } => None,
            // ConversationUpdated is visible to all workspace members.
            Self::ConversationUpdated { .. } => None,
            Self::TypingStarted {
                member_user_ids, ..
            } => Some(member_user_ids),
            Self::MessagePinned {
                member_user_ids, ..
            }
            | Self::MessageUnpinned {
                member_user_ids, ..
            } => Some(member_user_ids),
            // ReadStateUpdated is filtered by user_id in the subscription, not by member list.
            Self::ReadStateUpdated { .. } => None,
            // PresenceChanged is visible to all workspace members.
            Self::PresenceChanged { .. } => None,
            // UserMentioned is filtered per-user in the subscription.
            Self::UserMentioned {
                mentioned_user_ids, ..
            } => Some(mentioned_user_ids),
            // LinkPreviewReady is visible to all workspace members.
            Self::LinkPreviewReady { .. } => None,
            // WorkspaceState events: server-scope visible to all, user-scope only to owning user.
            Self::WorkspaceStateChanged { scope, user_id, .. }
            | Self::WorkspaceStateDeleted { scope, user_id, .. } => match scope {
                WorkspaceStateScopeDto::Server => None,
                WorkspaceStateScopeDto::User => user_id.as_ref().map(std::slice::from_ref),
            },
            // Pipeline events are visible to all workspace members.
            Self::PipelineFired { .. } | Self::PipelineSuspended { .. } => None,
            Self::IncidentDeclared {
                member_user_ids, ..
            }
            | Self::IncidentUpdated {
                member_user_ids, ..
            }
            | Self::EscalationStepFired {
                member_user_ids, ..
            }
            | Self::IncidentAcknowledged {
                member_user_ids, ..
            } => Some(member_user_ids),
        }
    }

    /// Returns `true` for events emitted by the pipeline evaluator itself.
    /// Used to prevent infinite loops in the evaluator task.
    pub fn is_pipeline_event(&self) -> bool {
        matches!(
            self,
            Self::PipelineFired { .. } | Self::PipelineSuspended { .. }
        )
    }
}
