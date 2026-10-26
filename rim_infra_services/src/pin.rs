use std::sync::Arc;

use async_trait::async_trait;
use tracing::{info, instrument};

use rim_core::message::model::MessageId;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::message::dto::MessageDto;
use rim_services::message::service::MessageService;
use rim_services::pin::command::{ListPinnedMessagesQuery, PinMessageCmd, UnpinMessageCmd};
use rim_services::pin::service::PinService;
use rim_services::{ServiceError, ServiceResult};

use crate::PinRepos;
use crate::conversation::manages_channel;
use crate::util::{
    ConversationAccessRepos, PostKind, ensure_can_post, ensure_conversation_member_access,
    ensure_conversation_writable_with_standing,
};

const MAX_PINS_PER_CONVERSATION: u64 = 50;

#[derive(Clone)]
pub struct DefaultPinService {
    repos: PinRepos,
    observers: Observers,
    message_service: Arc<dyn MessageService>,
}

impl DefaultPinService {
    pub fn new(
        repos: PinRepos,
        observers: Observers,
        message_service: Arc<dyn MessageService>,
    ) -> Self {
        Self {
            repos,
            observers,
            message_service,
        }
    }

    /// `NotFound` unless `message_id` names a message of `conversation_id`.
    async fn ensure_message_in_conversation(
        &self,
        message_id: MessageId,
        conversation_id: rim_core::conversation::model::ConversationId,
    ) -> ServiceResult<()> {
        let message = self
            .repos
            .message_repo
            .get_by_id(message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if message.conversation_id != conversation_id {
            return Err(ServiceError::NotFound);
        }
        Ok(())
    }
}

#[async_trait]
impl PinService for DefaultPinService {
    #[instrument(
        name = "pin.pin_message",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id, message_id = %cmd.message_id)
    )]
    async fn pin_message(&self, cmd: PinMessageCmd) -> ServiceResult<()> {
        let (conversation, standing) = ensure_conversation_writable_with_standing(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            cmd.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;
        ensure_can_post(&conversation, &standing, PostKind::Pin)?;

        self.ensure_message_in_conversation(cmd.message_id, cmd.conversation_id)
            .await?;

        // Check already pinned
        let already = self
            .repos
            .message_pin_repo
            .is_pinned(cmd.message_id)
            .await?;
        if already {
            return Err(ServiceError::Conflict(
                "message is already pinned".to_string(),
            ));
        }

        // Check max pins
        let count = self
            .repos
            .message_pin_repo
            .count_pins(cmd.conversation_id)
            .await?;
        if count >= MAX_PINS_PER_CONVERSATION {
            return Err(ServiceError::Validation(format!(
                "conversation already has the maximum of {} pinned messages",
                MAX_PINS_PER_CONVERSATION
            )));
        }

        self.repos
            .message_pin_repo
            .pin(cmd.message_id, cmd.conversation_id, cmd.actor.user_id)
            .await?;

        info!(
            message_id = %cmd.message_id,
            conversation_id = %cmd.conversation_id,
            "pinned message"
        );

        let member_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(cmd.conversation_id)
            .await?;

        self.observers.emit(WorkspaceEvent::MessagePinned {
            workspace_id: cmd.server_id,
            conversation_id: cmd.conversation_id,
            message_id: cmd.message_id,
            pinned_by: cmd.actor.user_id,
            conversation_type: conversation.conversation_type.as_str().to_string(),
            is_private: conversation.is_private,
            member_user_ids: member_ids,
        });

        Ok(())
    }

    #[instrument(
        name = "pin.unpin_message",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id, message_id = %cmd.message_id)
    )]
    async fn unpin_message(&self, cmd: UnpinMessageCmd) -> ServiceResult<()> {
        let (conversation, standing) = ensure_conversation_writable_with_standing(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            cmd.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;
        ensure_can_post(&conversation, &standing, PostKind::Pin)?;
        self.ensure_message_in_conversation(cmd.message_id, cmd.conversation_id)
            .await?;
        if !manages_channel(&conversation, standing.role, cmd.actor.user_id) {
            let pinned_by = self
                .repos
                .message_pin_repo
                .list_pinned(cmd.conversation_id)
                .await?
                .into_iter()
                .find(|pin| pin.message_id == cmd.message_id)
                .map(|pin| pin.pinned_by);
            if pinned_by.is_some_and(|by| by != cmd.actor.user_id) {
                return Err(ServiceError::Forbidden(
                    "only the member who pinned it, an owner or a moderator can unpin this \
                     message"
                        .to_string(),
                ));
            }
        }

        self.repos.message_pin_repo.unpin(cmd.message_id).await?;

        info!(
            message_id = %cmd.message_id,
            conversation_id = %cmd.conversation_id,
            "unpinned message"
        );

        let member_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(cmd.conversation_id)
            .await?;

        self.observers.emit(WorkspaceEvent::MessageUnpinned {
            workspace_id: cmd.server_id,
            conversation_id: cmd.conversation_id,
            message_id: cmd.message_id,
            conversation_type: conversation.conversation_type.as_str().to_string(),
            is_private: conversation.is_private,
            member_user_ids: member_ids,
        });

        Ok(())
    }

    #[instrument(
        name = "pin.list_pinned_messages",
        skip_all,
        fields(actor_id = %query.actor.user_id, server_id = %query.server_id, conversation_id = %query.conversation_id)
    )]
    async fn list_pinned_messages(
        &self,
        query: ListPinnedMessagesQuery,
    ) -> ServiceResult<Vec<MessageDto>> {
        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            query.actor.user_id,
            query.server_id,
            query.conversation_id,
        )
        .await?;

        let pins = self
            .repos
            .message_pin_repo
            .list_pinned(query.conversation_id)
            .await?;

        let message_ids: Vec<MessageId> = pins.iter().map(|p| p.message_id).collect();

        let messages = self
            .message_service
            .get_messages_by_ids(&message_ids)
            .await?;

        Ok(messages)
    }

    async fn is_pinned_batch(&self, message_ids: &[MessageId]) -> ServiceResult<Vec<MessageId>> {
        self.repos
            .message_pin_repo
            .is_pinned_batch(message_ids)
            .await
            .map_err(ServiceError::from)
    }
}
