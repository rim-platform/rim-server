use std::sync::Arc;

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

    #[allow(clippy::too_many_arguments)]
    pub fn on_message_sent(
        &self,
        workspace_id: &str,
        conversation_id: &str,
        conversation_type: &str,
        member_user_ids: Vec<String>,
        message: MessageEventData,
    ) {
        self.notify(WorkspaceEvent::NewMessage {
            workspace_id: workspace_id.to_string(),
            conversation_id: conversation_id.to_string(),
            conversation_type: conversation_type.to_string(),
            member_user_ids,
            message,
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn on_message_updated(
        &self,
        workspace_id: &str,
        conversation_id: &str,
        conversation_type: &str,
        member_user_ids: Vec<String>,
        message: MessageEventData,
    ) {
        self.notify(WorkspaceEvent::MessageUpdated {
            workspace_id: workspace_id.to_string(),
            conversation_id: conversation_id.to_string(),
            conversation_type: conversation_type.to_string(),
            member_user_ids,
            message,
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn on_message_deleted(
        &self,
        workspace_id: &str,
        conversation_id: &str,
        conversation_type: &str,
        member_user_ids: Vec<String>,
        message_id: &str,
    ) {
        self.notify(WorkspaceEvent::MessageDeleted {
            workspace_id: workspace_id.to_string(),
            conversation_id: conversation_id.to_string(),
            conversation_type: conversation_type.to_string(),
            member_user_ids,
            message_id: message_id.to_string(),
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

    #[allow(clippy::too_many_arguments)]
    pub fn on_reactions_changed(
        &self,
        workspace_id: &str,
        conversation_id: &str,
        conversation_type: &str,
        member_user_ids: Vec<String>,
        message_id: &str,
        reactions: Vec<ReactionGroupData>,
    ) {
        self.notify(WorkspaceEvent::ReactionsUpdated {
            workspace_id: workspace_id.to_string(),
            conversation_id: conversation_id.to_string(),
            conversation_type: conversation_type.to_string(),
            member_user_ids,
            message_id: message_id.to_string(),
            reactions,
        });
    }
}

impl EventObserver for ReactionObserver {
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
        workspace_id: &str,
        conversation_id: &str,
        title: Option<&str>,
        conversation_type: &str,
        member_user_ids: Vec<String>,
    ) {
        self.notify(WorkspaceEvent::ConversationCreated {
            workspace_id: workspace_id.to_string(),
            conversation_id: conversation_id.to_string(),
            title: title.map(|t| t.to_string()),
            conversation_type: conversation_type.to_string(),
            member_user_ids,
        });
    }

    pub fn on_member_joined(&self, workspace_id: &str, user_id: &str, username: &str) {
        self.notify(WorkspaceEvent::MemberJoined {
            workspace_id: workspace_id.to_string(),
            user_id: user_id.to_string(),
            username: username.to_string(),
        });
    }

    pub fn on_member_added_to_conversation(
        &self,
        workspace_id: &str,
        conversation_id: &str,
        user_id: &str,
    ) {
        self.notify(WorkspaceEvent::MemberAddedToConversation {
            workspace_id: workspace_id.to_string(),
            conversation_id: conversation_id.to_string(),
            user_id: user_id.to_string(),
        });
    }

    pub fn on_member_removed_from_conversation(
        &self,
        workspace_id: &str,
        conversation_id: &str,
        user_id: &str,
    ) {
        self.notify(WorkspaceEvent::MemberRemovedFromConversation {
            workspace_id: workspace_id.to_string(),
            conversation_id: conversation_id.to_string(),
            user_id: user_id.to_string(),
        });
    }
}

impl EventObserver for WorkspaceObserver {
    fn broadcaster(&self) -> &Arc<dyn Broadcaster> {
        &self.broadcaster
    }
}

/// Container for all observers — passed to services that emit events.
#[derive(Clone)]
pub struct Observers {
    pub message: Arc<MessageObserver>,
    pub reaction: Arc<ReactionObserver>,
    pub workspace: Arc<WorkspaceObserver>,
}

impl Observers {
    pub fn new(broadcaster: Arc<dyn Broadcaster>) -> Self {
        Self {
            message: Arc::new(MessageObserver::new(broadcaster.clone())),
            reaction: Arc::new(ReactionObserver::new(broadcaster.clone())),
            workspace: Arc::new(WorkspaceObserver::new(broadcaster)),
        }
    }
}
