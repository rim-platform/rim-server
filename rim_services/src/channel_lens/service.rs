use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::channel_lens::command::{
    ApplyLensQuery, CreateLensCmd, DeleteLensCmd, ListLensesQuery, UpdateLensCmd,
};
use crate::channel_lens::dto::ChannelLensDto;
use crate::message::dto::MessageDto;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ChannelLensService: Send + Sync {
    async fn create_lens(&self, cmd: CreateLensCmd) -> ServiceResult<ChannelLensDto>;

    async fn update_lens(&self, cmd: UpdateLensCmd) -> ServiceResult<ChannelLensDto>;

    async fn delete_lens(&self, cmd: DeleteLensCmd) -> ServiceResult<()>;

    async fn list_lenses(&self, query: ListLensesQuery) -> ServiceResult<Vec<ChannelLensDto>>;

    async fn apply_lens(&self, query: ApplyLensQuery) -> ServiceResult<Vec<MessageDto>>;
}
