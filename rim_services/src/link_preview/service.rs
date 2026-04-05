use std::collections::HashMap;

use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::link_preview::dto::LinkPreviewDto;
use crate::ServiceResult;
use rim_core::message::model::MessageId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait LinkPreviewService: Send + Sync {
    async fn fetch_and_store(
        &self,
        message_id: MessageId,
        urls: Vec<String>,
    ) -> ServiceResult<Vec<LinkPreviewDto>>;

    async fn get_previews_for_message(
        &self,
        message_id: MessageId,
    ) -> ServiceResult<Vec<LinkPreviewDto>>;

    async fn get_previews_for_messages(
        &self,
        message_ids: &[MessageId],
    ) -> ServiceResult<HashMap<MessageId, Vec<LinkPreviewDto>>>;
}
