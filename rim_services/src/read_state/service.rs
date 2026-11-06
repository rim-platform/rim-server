use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::read_state::dto::{
    MarkAllAsReadCmd, MarkAsReadCmd, MarkAsUnreadCmd, UnreadCountDto, UnreadCountsQuery,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ReadStateService: Send + Sync {
    async fn mark_as_read(&self, cmd: MarkAsReadCmd) -> ServiceResult<()>;

    /// Moves the actor's read marker back so that the message and every later one count as
    /// unread. A marker already before the message stays. Notifications already read stay read.
    ///
    /// # Errors
    /// The errors of a read of the conversation; `NotFound` for a message that does not exist, is
    /// deleted or is in another conversation; `Validation` for a thread reply, since replies are
    /// not counted as unread.
    async fn mark_as_unread(&self, cmd: MarkAsUnreadCmd) -> ServiceResult<()>;

    async fn mark_all_as_read(&self, cmd: MarkAllAsReadCmd) -> ServiceResult<()>;

    async fn get_unread_counts(
        &self,
        query: UnreadCountsQuery,
    ) -> ServiceResult<Vec<UnreadCountDto>>;
}
