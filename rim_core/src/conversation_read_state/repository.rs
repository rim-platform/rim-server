use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::conversation::model::ConversationId;
use crate::conversation_read_state::model::ConversationReadState;
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ConversationReadStateRepository: Send + Sync {
    async fn upsert(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
        last_read_at: DateTime<Utc>,
    ) -> RepoResult<()>;

    async fn get(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
    ) -> RepoResult<Option<ConversationReadState>>;

    async fn get_unread_counts(
        &self,
        user_id: UserId,
        conversation_ids: &[ConversationId],
    ) -> RepoResult<Vec<(ConversationId, i64)>>;

    async fn mark_all_read(
        &self,
        user_id: UserId,
        conversation_ids: &[ConversationId],
        read_at: DateTime<Utc>,
    ) -> RepoResult<()>;
}
