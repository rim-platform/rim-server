use std::sync::Arc;

use async_trait::async_trait;
use tracing::{info, instrument};

use rim_core::message::model::MessageId;
use rim_services::events::Observers;
use rim_services::message::dto::MessageDto;
use rim_services::message::service::MessageService;
use rim_services::pin::command::{ListPinnedMessagesQuery, PinMessageCmd, UnpinMessageCmd};
use rim_services::pin::service::PinService;
use rim_services::{ServiceError, ServiceResult};

use crate::PinRepos;
use crate::util::{ensure_conversation_access, ensure_conversation_in_server, ensure_server_member};

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
}

#[async_trait]
impl PinService for DefaultPinService {
    #[instrument(
        name = "pin.pin_message",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id, message_id = %cmd.message_id)
    )]
    async fn pin_message(&self, cmd: PinMessageCmd) -> ServiceResult<()> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let conversation = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conversation,
            cmd.actor.user_id,
        )
        .await?;

        // Check message exists and belongs to this conversation
        let message = self
            .repos
            .message_repo
            .get_by_id(cmd.message_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if message.conversation_id != cmd.conversation_id {
            return Err(ServiceError::Validation(
                "message does not belong to this conversation".to_string(),
            ));
        }

        // Check already pinned
        let already = self
            .repos
            .message_pin_repo
            .is_pinned(cmd.message_id)
            .await
            .map_err(rim_services::map_repo_error)?;
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
            .await
            .map_err(rim_services::map_repo_error)?;
        if count >= MAX_PINS_PER_CONVERSATION {
            return Err(ServiceError::Validation(format!(
                "conversation already has the maximum of {} pinned messages",
                MAX_PINS_PER_CONVERSATION
            )));
        }

        self.repos
            .message_pin_repo
            .pin(cmd.message_id, cmd.conversation_id, cmd.actor.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(
            message_id = %cmd.message_id,
            conversation_id = %cmd.conversation_id,
            "pinned message"
        );

        let member_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(cmd.conversation_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        self.observers.workspace.on_message_pinned(
            cmd.server_id,
            cmd.conversation_id,
            cmd.message_id,
            cmd.actor.user_id,
            member_ids,
        );

        Ok(())
    }

    #[instrument(
        name = "pin.unpin_message",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id, message_id = %cmd.message_id)
    )]
    async fn unpin_message(&self, cmd: UnpinMessageCmd) -> ServiceResult<()> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let conversation = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conversation,
            cmd.actor.user_id,
        )
        .await?;

        self.repos
            .message_pin_repo
            .unpin(cmd.message_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(
            message_id = %cmd.message_id,
            conversation_id = %cmd.conversation_id,
            "unpinned message"
        );

        let member_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(cmd.conversation_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        self.observers.workspace.on_message_unpinned(
            cmd.server_id,
            cmd.conversation_id,
            cmd.message_id,
            member_ids,
        );

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
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let conversation = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            query.conversation_id,
            query.server_id,
        )
        .await?;

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conversation,
            query.actor.user_id,
        )
        .await?;

        let pins = self
            .repos
            .message_pin_repo
            .list_pinned(query.conversation_id)
            .await
            .map_err(rim_services::map_repo_error)?;

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
            .map_err(rim_services::map_repo_error)
    }
}
