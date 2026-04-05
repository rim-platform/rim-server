use std::sync::Arc;

use chrono::{DateTime, Utc};
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use super::broadcast::Broadcaster;
use super::types::{MessageEventData, ReactionGroupData, WorkspaceEvent};

/// Base trait for all observers — provides notify() which forwards to broadcaster.
pub trait EventObserver: Send + Sync {
    fn broadcaster(&self) -> &Arc<dyn Broadcaster>;

    fn notify(&self, event: WorkspaceEvent) {
        self.broadcaster().broadcast(event);
    }
}

/// Observer for message-related events.
pub struct MessageObserver {
    broadcaster: Arc<dyn Broadcaster>,
}

impl MessageObserver {
    pub fn new(broadcaster: Arc<dyn Broadcaster>) -> Self {
        Self { broadcaster }
    }

    pub fn on_message_sent(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: &str,
        member_user_ids: Vec<UserId>,
        message: MessageEventData,
    ) {
        self.notify(WorkspaceEvent::NewMessage {
            workspace_id,
            conversation_id,
            conversation_type: conversation_type.to_string(),
            member_user_ids,
            message,
        });
    }

    pub fn on_message_updated(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: &str,
        member_user_ids: Vec<UserId>,
        message: MessageEventData,
    ) {
        self.notify(WorkspaceEvent::MessageUpdated {
            workspace_id,
            conversation_id,
            conversation_type: conversation_type.to_string(),
            member_user_ids,
            message,
        });
    }

    pub fn on_message_deleted(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: &str,
        member_user_ids: Vec<UserId>,
        message_id: MessageId,
        thread_id: Option<MessageThreadId>,
    ) {
        self.notify(WorkspaceEvent::MessageDeleted {
            workspace_id,
            conversation_id,
            conversation_type: conversation_type.to_string(),
            member_user_ids,
            message_id,
            thread_id,
        });
    }

}

impl EventObserver for MessageObserver {
    fn broadcaster(&self) -> &Arc<dyn Broadcaster> {
        &self.broadcaster
    }
}

/// Observer for reaction-related events.
pub struct ReactionObserver {
    broadcaster: Arc<dyn Broadcaster>,
}

impl ReactionObserver {
    pub fn new(broadcaster: Arc<dyn Broadcaster>) -> Self {
        Self { broadcaster }
    }

    pub fn on_reactions_changed(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: &str,
        member_user_ids: Vec<UserId>,
        message_id: MessageId,
        reactions: Vec<ReactionGroupData>,
    ) {
        self.notify(WorkspaceEvent::ReactionsUpdated {
            workspace_id,
            conversation_id,
            conversation_type: conversation_type.to_string(),
            member_user_ids,
            message_id,
            reactions,
        });
    }
}

impl EventObserver for ReactionObserver {
    fn broadcaster(&self) -> &Arc<dyn Broadcaster> {
        &self.broadcaster
    }
}

/// Observer for thread-related events.
pub struct ThreadObserver {
    broadcaster: Arc<dyn Broadcaster>,
}

impl ThreadObserver {
    pub fn new(broadcaster: Arc<dyn Broadcaster>) -> Self {
        Self { broadcaster }
    }

    pub fn on_thread_created(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
        thread_id: MessageThreadId,
        root_message_id: MessageId,
        conversation_type: &str,
        member_user_ids: Vec<UserId>,
    ) {
        self.notify(WorkspaceEvent::ThreadCreated {
            workspace_id,
            conversation_id,
            thread_id,
            root_message_id,
            conversation_type: conversation_type.to_string(),
            member_user_ids,
        });
    }
}

impl EventObserver for ThreadObserver {
    fn broadcaster(&self) -> &Arc<dyn Broadcaster> {
        &self.broadcaster
    }
}

/// Observer for conversation/workspace events.
pub struct WorkspaceObserver {
    broadcaster: Arc<dyn Broadcaster>,
}

impl WorkspaceObserver {
    pub fn new(broadcaster: Arc<dyn Broadcaster>) -> Self {
        Self { broadcaster }
    }

    pub fn on_conversation_created(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
        title: Option<&str>,
        conversation_type: &str,
        member_user_ids: Vec<UserId>,
    ) {
        self.notify(WorkspaceEvent::ConversationCreated {
            workspace_id,
            conversation_id,
            title: title.map(|t| t.to_string()),
            conversation_type: conversation_type.to_string(),
            member_user_ids,
        });
    }

    pub fn on_member_joined(&self, workspace_id: ServerId, user_id: UserId, username: &str) {
        self.notify(WorkspaceEvent::MemberJoined {
            workspace_id,
            user_id,
            username: username.to_string(),
        });
    }

    pub fn on_message_pinned(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
        message_id: MessageId,
        pinned_by: UserId,
        member_user_ids: Vec<UserId>,
    ) {
        self.notify(WorkspaceEvent::MessagePinned {
            workspace_id,
            conversation_id,
            message_id,
            pinned_by,
            member_user_ids,
        });
    }

    pub fn on_message_unpinned(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
        message_id: MessageId,
        member_user_ids: Vec<UserId>,
    ) {
        self.notify(WorkspaceEvent::MessageUnpinned {
            workspace_id,
            conversation_id,
            message_id,
            member_user_ids,
        });
    }

    pub fn on_typing_started(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
        conversation_type: String,
        member_user_ids: Vec<UserId>,
    ) {
        self.notify(WorkspaceEvent::TypingStarted {
            workspace_id,
            conversation_id,
            user_id,
            conversation_type,
            member_user_ids,
        });
    }

    pub fn on_conversation_updated(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
    ) {
        self.notify(WorkspaceEvent::ConversationUpdated {
            workspace_id,
            conversation_id,
        });
    }

    pub fn on_member_added_to_conversation(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
        member_user_ids: Vec<UserId>,
    ) {
        self.notify(WorkspaceEvent::MemberAddedToConversation {
            workspace_id,
            conversation_id,
            user_id,
            member_user_ids,
        });
    }

    pub fn on_member_removed_from_conversation(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
        member_user_ids: Vec<UserId>,
    ) {
        self.notify(WorkspaceEvent::MemberRemovedFromConversation {
            workspace_id,
            conversation_id,
            user_id,
            member_user_ids,
        });
    }
}

impl EventObserver for WorkspaceObserver {
    fn broadcaster(&self) -> &Arc<dyn Broadcaster> {
        &self.broadcaster
    }
}

/// Observer for read-state events.
pub struct ReadStateObserver {
    broadcaster: Arc<dyn Broadcaster>,
}

impl ReadStateObserver {
    pub fn new(broadcaster: Arc<dyn Broadcaster>) -> Self {
        Self { broadcaster }
    }

    pub fn on_read_state_updated(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
        last_read_at: DateTime<Utc>,
    ) {
        self.notify(WorkspaceEvent::ReadStateUpdated {
            workspace_id,
            conversation_id,
            user_id,
            last_read_at,
        });
    }
}

impl EventObserver for ReadStateObserver {
    fn broadcaster(&self) -> &Arc<dyn Broadcaster> {
        &self.broadcaster
    }
}

/// Observer for mention-related events.
pub struct MentionObserver {
    broadcaster: Arc<dyn Broadcaster>,
}

impl MentionObserver {
    pub fn new(broadcaster: Arc<dyn Broadcaster>) -> Self {
        Self { broadcaster }
    }

    pub fn on_user_mentioned(
        &self,
        workspace_id: ServerId,
        conversation_id: ConversationId,
        message_id: MessageId,
        mentioned_user_ids: Vec<UserId>,
        author_id: UserId,
    ) {
        self.notify(WorkspaceEvent::UserMentioned {
            workspace_id,
            conversation_id,
            message_id,
            mentioned_user_ids,
            author_id,
        });
    }
}

impl EventObserver for MentionObserver {
    fn broadcaster(&self) -> &Arc<dyn Broadcaster> {
        &self.broadcaster
    }
}

/// Container for all observers — passed to services that emit events.
#[derive(Clone)]
pub struct Observers {
    pub message: Arc<MessageObserver>,
    pub reaction: Arc<ReactionObserver>,
    pub thread: Arc<ThreadObserver>,
    pub workspace: Arc<WorkspaceObserver>,
    pub read_state: Arc<ReadStateObserver>,
    pub mention: Arc<MentionObserver>,
}

impl Observers {
    pub fn new(broadcaster: Arc<dyn Broadcaster>) -> Self {
        Self {
            message: Arc::new(MessageObserver::new(broadcaster.clone())),
            reaction: Arc::new(ReactionObserver::new(broadcaster.clone())),
            thread: Arc::new(ThreadObserver::new(broadcaster.clone())),
            workspace: Arc::new(WorkspaceObserver::new(broadcaster.clone())),
            read_state: Arc::new(ReadStateObserver::new(broadcaster.clone())),
            mention: Arc::new(MentionObserver::new(broadcaster)),
        }
    }
}
