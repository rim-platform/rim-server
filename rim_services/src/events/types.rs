use serde::{Deserialize, Serialize};

use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::notification::model::UserNotificationId;
use rim_core::pipeline::model::PipelineId;
use rim_core::public_join_link::model::JoinRequestId;
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
        /// Server-side only (never serialized): a private channel's events reach its members
        /// alone, so the subscription filter must not treat it like a public channel.
        #[serde(skip_serializing, default)]
        is_private: bool,
        message: MessageEventData,
    },
    MessageUpdated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
        /// Server-side only (never serialized): a private channel's events reach its members
        /// alone, so the subscription filter must not treat it like a public channel.
        #[serde(skip_serializing, default)]
        is_private: bool,
        message: MessageEventData,
    },
    MessageDeleted {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
        /// Server-side only (never serialized): a private channel's events reach its members
        /// alone, so the subscription filter must not treat it like a public channel.
        #[serde(skip_serializing, default)]
        is_private: bool,
        message_id: MessageId,
        thread_id: Option<MessageThreadId>,
    },
    ReactionsUpdated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
        /// Server-side only (never serialized): a private channel's events reach its members
        /// alone, so the subscription filter must not treat it like a public channel.
        #[serde(skip_serializing, default)]
        is_private: bool,
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
        /// Server-side only (never serialized): a private channel's events reach its members
        /// alone, so the subscription filter must not treat it like a public channel.
        #[serde(skip_serializing, default)]
        is_private: bool,
    },
    /// A profile changed; clients refetch the member directory and their own profile.
    MemberProfileUpdated {
        workspace_id: ServerId,
        user_id: UserId,
    },
    /// A workspace ban changed; clients refetch author status without receiving its reason.
    MemberBanChanged {
        workspace_id: ServerId,
        user_id: UserId,
    },
    MemberJoined {
        workspace_id: ServerId,
        user_id: UserId,
        username: String,
        /// How they joined, when it was through the workspace's public link: `public_link`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    /// A person asked to join the workspace through its public link. Wire shape:
    /// `{"type":"JoinRequestCreated","workspace_id","request_id","user_id"}`. Reaches the
    /// workspace's Owner and Moderators, and the person on their direct-space subscription.
    JoinRequestCreated {
        workspace_id: ServerId,
        request_id: JoinRequestId,
        user_id: UserId,
        /// Server-side only: the workspace's Owner and Moderators, and the person.
        #[serde(skip_serializing, default)]
        member_user_ids: Vec<UserId>,
    },
    /// A join request was closed. `outcome` is `accepted`, `declined`, `withdrawn` or
    /// `cancelled`. Wire shape:
    /// `{"type":"JoinRequestDecided","workspace_id","request_id","user_id","outcome"}`. Same
    /// recipients as `JoinRequestCreated`.
    JoinRequestDecided {
        workspace_id: ServerId,
        request_id: JoinRequestId,
        user_id: UserId,
        outcome: String,
        /// Server-side only: the workspace's Owner and Moderators, and the person.
        #[serde(skip_serializing, default)]
        member_user_ids: Vec<UserId>,
    },
    /// A member left the workspace because a moderator kicked or banned them.
    /// Wire shape: `{"type":"MemberLeft","workspace_id","user_id"}`. Delivered to every
    /// subscriber of the workspace, including the removed user, whose subscription then ends.
    MemberLeft {
        workspace_id: ServerId,
        user_id: UserId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cause: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<chrono::DateTime<chrono::Utc>>,
        /// Private to the removed user's own delivery.
        #[serde(default, skip_serializing)]
        reason: Option<String>,
    },
    /// A member's role changed. `role` is the snake_case role name (`owner`,
    /// `moderator`, `member`, `guest`). Wire shape:
    /// `{"type":"MemberRoleChanged","workspace_id","user_id","role"}`.
    MemberRoleChanged {
        workspace_id: ServerId,
        user_id: UserId,
        role: String,
    },
    ThreadCreated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        thread_id: MessageThreadId,
        root_message_id: MessageId,
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
        /// Server-side only (never serialized): a private channel's events reach its members
        /// alone, so the subscription filter must not treat it like a public channel.
        #[serde(skip_serializing, default)]
        is_private: bool,
    },
    MemberAddedToConversation {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
        /// Server-side only: who added them, the user themself for a join; `None` when they
        /// accepted an invitation.
        #[serde(skip_serializing, default)]
        added_by: Option<UserId>,
    },
    MemberRemovedFromConversation {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    /// Topic or archive state changed. The visibility fields are server-side only (never
    /// serialized, so the wire shape is unchanged): public-channel updates reach every workspace
    /// member, everything else only the conversation's members.
    ConversationUpdated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        #[serde(skip_serializing)]
        conversation_type: String,
        #[serde(skip_serializing, default)]
        is_private: bool,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    /// Carries `conversation_type` (like `TypingStarted`) so the subscription filter can treat
    /// public channels as visible to all workspace members without relying on
    /// `member_user_ids` — a public channel's member list only covers those who joined it.
    /// `skip_serializing` keeps the client wire shape unchanged: this field
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
        /// Server-side only (never serialized): a private channel's events reach its members
        /// alone, so the subscription filter must not treat it like a public channel.
        #[serde(skip_serializing, default)]
        is_private: bool,
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
        /// Server-side only (never serialized): a private channel's events reach its members
        /// alone, so the subscription filter must not treat it like a public channel.
        #[serde(skip_serializing, default)]
        is_private: bool,
    },
    /// A member started typing; names them so the client can show who without a lookup.
    TypingStarted {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
        /// The typing user's username.
        username: String,
        /// The typing user's display name, `null` when they have none.
        display_name: Option<String>,
        #[serde(skip_serializing)]
        conversation_type: String,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
        /// Server-side only (never serialized): a private channel's events reach its members
        /// alone, so the subscription filter must not treat it like a public channel.
        #[serde(skip_serializing, default)]
        is_private: bool,
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
    /// See [`Self::ConversationUpdated`] for the server-side-only visibility fields.
    LinkPreviewReady {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        message_id: MessageId,
        #[serde(skip_serializing)]
        conversation_type: String,
        #[serde(skip_serializing, default)]
        is_private: bool,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    /// A workspace state key was written. The value is not carried: a client that shows it
    /// reads it again.
    WorkspaceStateChanged {
        workspace_id: ServerId,
        key: String,
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
    /// A pipeline ran its actions. Delivered to its creator only (`member_user_ids ==
    /// [created_by]`).
    PipelineFired {
        workspace_id: ServerId,
        pipeline_id: PipelineId,
        pipeline_name: String,
        trigger_event: String,
        actions_executed: usize,
        #[serde(skip_serializing, default)]
        member_user_ids: Vec<UserId>,
    },
    /// A pipeline was switched off for firing too often. Delivered to its creator only
    /// (`member_user_ids == [created_by]`).
    PipelineSuspended {
        workspace_id: ServerId,
        pipeline_id: PipelineId,
        pipeline_name: String,
        reason: String,
        #[serde(skip_serializing, default)]
        member_user_ids: Vec<UserId>,
    },
    /// The server decided to notify `user_id` about a message: emitted once per
    /// recipient after the notification row is stored. Delivered to that user only
    /// (`member_user_ids == [user_id]`). `reason` is the snake_case notification reason
    /// (`direct`, `mention`, `keyword`, `thread_reply`, `all`, `reaction`); `quiet` is true
    /// inside the user's do-not-disturb window or pause, so clients record it without a toast
    /// or sound. `reaction_emoji` and `reaction_by` (the reactor) are present for `reaction`
    /// only.
    NotificationDue {
        workspace_id: ServerId,
        user_id: UserId,
        notification_id: UserNotificationId,
        conversation_id: ConversationId,
        message_id: MessageId,
        reason: String,
        quiet: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        reaction_emoji: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reaction_by: Option<UserId>,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    /// An entry of the workspace's plugin registry was added, updated or removed. Visible to
    /// every member, guests included; it carries no entry data, so clients refetch
    /// `workspacePlugins`.
    WorkspacePluginChanged {
        workspace_id: ServerId,
        plugin_id: String,
        change: WorkspacePluginChange,
    },
    /// The server accepted a Commit of an encrypted conversation's group. `epoch` is the epoch
    /// the Commit was created in. Delivered to the conversation's members.
    MlsCommitAccepted {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        incarnation: i32,
        epoch: i64,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    /// The devices an encrypted conversation's group should add or remove changed: membership,
    /// devices, key package activity or re-add requests. Clients refetch `Conversation.mlsGroup`.
    MlsMembershipChanged {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
    /// A Welcome waits for one of `user_id`'s devices; other devices ignore it. Delivered to
    /// `user_id` only.
    MlsWelcomeAvailable {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
        /// The client device id.
        device_id: uuid::Uuid,
        #[serde(skip_serializing)]
        member_user_ids: Vec<UserId>,
    },
}

/// What happened to a registry entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspacePluginChange {
    Added,
    Updated,
    Removed,
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
    /// When the text last changed; null for a never-edited or deleted message.
    pub edited_at: Option<chrono::DateTime<chrono::Utc>>,
    pub reactions: Vec<ReactionGroupData>,
    /// The envelope of an encrypted message; `None` for plaintext.
    pub encryption: Option<MessageEncryptionData>,
}

/// An encrypted message's envelope in events. Binary values are unpadded base64url; `sent_at_us`
/// is decimal microseconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEncryptionData {
    pub incarnation: i32,
    pub epoch: i64,
    pub sender_device_id: uuid::Uuid,
    pub ciphertext: Option<String>,
    pub franking_receipt: String,
    pub franking_key_id: i32,
    pub sent_at_us: String,
}

impl From<&crate::message::dto::MessageEncryptionDto> for MessageEncryptionData {
    fn from(dto: &crate::message::dto::MessageEncryptionDto) -> Self {
        use base64::Engine;
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        Self {
            incarnation: dto.incarnation,
            epoch: dto.epoch,
            sender_device_id: dto.sender_device_id,
            ciphertext: dto.ciphertext.as_deref().map(b64),
            franking_receipt: b64(&dto.franking_receipt),
            franking_key_id: dto.franking_key_id,
            sent_at_us: dto.sent_at_us.to_string(),
        }
    }
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
            edited_at: if dto.is_deleted { None } else { dto.edited_at },
            reactions: vec![],
            encryption: dto.encryption.as_ref().map(Into::into),
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
            | Self::MemberProfileUpdated { workspace_id, .. }
            | Self::MemberBanChanged { workspace_id, .. }
            | Self::MemberJoined { workspace_id, .. }
            | Self::JoinRequestCreated { workspace_id, .. }
            | Self::JoinRequestDecided { workspace_id, .. }
            | Self::MemberLeft { workspace_id, .. }
            | Self::MemberRoleChanged { workspace_id, .. }
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
            | Self::NotificationDue { workspace_id, .. }
            | Self::WorkspacePluginChanged { workspace_id, .. }
            | Self::MlsCommitAccepted { workspace_id, .. }
            | Self::MlsMembershipChanged { workspace_id, .. }
            | Self::MlsWelcomeAvailable { workspace_id, .. } => workspace_id.to_string(),
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
            Self::JoinRequestCreated {
                member_user_ids, ..
            }
            | Self::JoinRequestDecided {
                member_user_ids, ..
            } => Some(member_user_ids),
            Self::MemberProfileUpdated { .. }
            | Self::MemberBanChanged { .. }
            | Self::MemberJoined { .. }
            | Self::MemberLeft { .. }
            | Self::MemberRoleChanged { .. } => None,
            Self::ConversationUpdated {
                member_user_ids, ..
            } => Some(member_user_ids),
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
            Self::LinkPreviewReady {
                member_user_ids, ..
            } => Some(member_user_ids),
            // WorkspaceState events: server-scope visible to all, user-scope only to owning user.
            Self::WorkspaceStateChanged { scope, user_id, .. }
            | Self::WorkspaceStateDeleted { scope, user_id, .. } => match scope {
                WorkspaceStateScopeDto::Server => None,
                WorkspaceStateScopeDto::User => user_id.as_ref().map(std::slice::from_ref),
            },
            // Pipeline events are scoped to the pipeline's creator.
            Self::PipelineFired {
                member_user_ids, ..
            }
            | Self::PipelineSuspended {
                member_user_ids, ..
            } => Some(member_user_ids),
            // Plugin registry events are visible to all workspace members.
            Self::WorkspacePluginChanged { .. } => None,
            // NotificationDue is scoped to its one recipient.
            Self::NotificationDue {
                member_user_ids, ..
            } => Some(member_user_ids),
            // MLS events reach the conversation's members; a Welcome only its device's user.
            Self::MlsCommitAccepted {
                member_user_ids, ..
            }
            | Self::MlsMembershipChanged {
                member_user_ids, ..
            }
            | Self::MlsWelcomeAvailable {
                member_user_ids, ..
            } => Some(member_user_ids),
        }
    }

    /// The type of the conversation an event is about (`Direct`, `Group` or `Channel`), for the
    /// events that carry it.
    pub fn conversation_type(&self) -> Option<&str> {
        match self {
            Self::NewMessage {
                conversation_type, ..
            }
            | Self::MessageUpdated {
                conversation_type, ..
            }
            | Self::MessageDeleted {
                conversation_type, ..
            }
            | Self::ReactionsUpdated {
                conversation_type, ..
            }
            | Self::ConversationCreated {
                conversation_type, ..
            }
            | Self::ThreadCreated {
                conversation_type, ..
            }
            | Self::ConversationUpdated {
                conversation_type, ..
            }
            | Self::MessagePinned {
                conversation_type, ..
            }
            | Self::MessageUnpinned {
                conversation_type, ..
            }
            | Self::TypingStarted {
                conversation_type, ..
            }
            | Self::LinkPreviewReady {
                conversation_type, ..
            } => Some(conversation_type),
            _ => None,
        }
    }

    /// The person whose direct-space subscription also carries the event: a join request's
    /// person, who is not a member of the workspace the event is about.
    pub fn direct_space_recipient(&self) -> Option<UserId> {
        match self {
            Self::JoinRequestCreated { user_id, .. } | Self::JoinRequestDecided { user_id, .. } => {
                Some(*user_id)
            }
            _ => None,
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

impl WorkspaceEvent {
    /// The event's wire form for one subscriber, with removal reasons private to their subject.
    ///
    /// # Errors
    /// When an event cannot be serialized.
    pub fn json_for_user(&self, user_id: &str) -> Result<serde_json::Value, serde_json::Error> {
        let mut value = serde_json::to_value(self)?;
        if let Self::MemberLeft {
            user_id: removed,
            reason: Some(reason),
            ..
        } = self
            && removed.to_string() == user_id
        {
            value["reason"] = serde_json::Value::String(reason.clone());
        }
        Ok(value)
    }
}

#[cfg(test)]
mod removal_tests {
    use super::*;

    #[test]
    fn removal_reasons_are_serialized_only_for_the_removed_user() {
        let removed = UserId::new();
        let event = WorkspaceEvent::MemberLeft {
            workspace_id: ServerId::new(),
            user_id: removed,
            cause: Some("ban".to_string()),
            until: None,
            reason: Some("private explanation".to_string()),
        };
        assert!(
            serde_json::to_value(&event)
                .unwrap()
                .get("reason")
                .is_none()
        );
        let own = event.json_for_user(&removed.to_string()).unwrap();
        assert_eq!(own["reason"], "private explanation");
        assert_eq!(own["cause"], "ban");
        assert!(
            event
                .json_for_user(&UserId::new().to_string())
                .unwrap()
                .get("reason")
                .is_none()
        );
        let received: WorkspaceEvent = serde_json::from_value(own).unwrap();
        assert_eq!(
            received.json_for_user(&removed.to_string()).unwrap()["reason"],
            "private explanation"
        );
    }
}
