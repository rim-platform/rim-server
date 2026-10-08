use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::message::command::{
    DeleteMessageCmd, EditMessageCmd, ForwardMessageCmd, GetMessageQuery, GetMessageReferenceQuery,
    ListMessageChangesQuery, ListMessagesQuery, ListThreadMessagesQuery, ReplyToMessageCmd,
    SearchMessagesQuery, SendMessageCmd,
};
use crate::message::dto::{
    ConversationMessageChangesDto, MessageDto, MessagePageCursor, MessageReferenceDto,
    MessageSearchHitDto,
};
use rim_core::message::model::MessageId;
use rim_core::repository::pagination::Page;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MessageService: Send + Sync {
    async fn send_message(&self, cmd: SendMessageCmd) -> ServiceResult<MessageDto>;

    async fn reply_to_message(&self, cmd: ReplyToMessageCmd) -> ServiceResult<MessageDto>;

    async fn get_message(&self, query: GetMessageQuery) -> ServiceResult<MessageDto>;

    async fn get_message_reference(
        &self,
        query: GetMessageReferenceQuery,
    ) -> ServiceResult<MessageReferenceDto>;

    async fn list_messages(
        &self,
        query: ListMessagesQuery,
    ) -> ServiceResult<Page<MessageDto, MessagePageCursor>>;

    async fn list_thread_messages(
        &self,
        query: ListThreadMessagesQuery,
    ) -> ServiceResult<Page<MessageDto, MessagePageCursor>>;

    /// Full-text search, cursor-paginated by `(sent_at, id)` (see `docs/pagination.md`) and
    /// ranked via `rank`/`headline` on each hit. Visibility-scoped: never returns a message in a
    /// conversation the actor cannot see.
    async fn search_messages(
        &self,
        query: SearchMessagesQuery,
    ) -> ServiceResult<Page<MessageSearchHitDto, MessagePageCursor>>;

    /// The message change feed (`docs/message-change-feed.md`): for each requested
    /// conversation, the messages created, edited or deleted after its cursor, in version order.
    /// A conversation the actor cannot read, or whose cursor is ahead of its counter, is reported
    /// on its own entry (`ConversationChangesStatus`); only a non-member of the workspace fails.
    async fn list_message_changes(
        &self,
        query: ListMessageChangesQuery,
    ) -> ServiceResult<Vec<ConversationMessageChangesDto>>;

    async fn edit_message(&self, cmd: EditMessageCmd) -> ServiceResult<MessageDto>;

    async fn forward_message(&self, cmd: ForwardMessageCmd) -> ServiceResult<MessageDto>;

    async fn delete_message(&self, cmd: DeleteMessageCmd) -> ServiceResult<()>;

    async fn get_messages_by_ids(&self, ids: &[MessageId]) -> ServiceResult<Vec<MessageDto>>;
}
