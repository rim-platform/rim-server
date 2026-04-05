use std::collections::HashMap;

use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::link_preview::model::{LinkPreview, NewLinkPreview};
use crate::message::model::MessageId;
use crate::repository::error::RepoResult;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait LinkPreviewRepository: Send + Sync {
    async fn create(&self, preview: NewLinkPreview) -> RepoResult<LinkPreview>;

    async fn get_for_message(&self, message_id: MessageId) -> RepoResult<Vec<LinkPreview>>;

    async fn get_for_messages(
        &self,
        message_ids: &[MessageId],
    ) -> RepoResult<HashMap<MessageId, Vec<LinkPreview>>>;
}
