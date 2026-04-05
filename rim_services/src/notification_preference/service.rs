use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::notification_preference::command::{
    IsMutedQuery, ListMutedConversationsQuery, MuteConversationCmd, UnmuteConversationCmd,
};
use crate::notification_preference::dto::MutedConversationDto;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait NotificationPreferenceService: Send + Sync {
    async fn mute_conversation(&self, cmd: MuteConversationCmd) -> ServiceResult<bool>;

    async fn unmute_conversation(&self, cmd: UnmuteConversationCmd) -> ServiceResult<bool>;

    async fn is_muted(&self, query: IsMutedQuery) -> ServiceResult<bool>;

    async fn list_muted_conversations(
        &self,
        query: ListMutedConversationsQuery,
    ) -> ServiceResult<Vec<MutedConversationDto>>;
}
