use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::pipeline::model::{Pipeline, PipelineId};
use crate::repository::error::RepoResult;
use crate::server::model::ServerId;
use crate::user::model::UserId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait PipelineRepository: Send + Sync {
    async fn create(&self, pipeline: &Pipeline) -> RepoResult<()>;

    async fn get_by_id(&self, id: PipelineId) -> RepoResult<Option<Pipeline>>;

    async fn list_for_server(&self, server_id: ServerId) -> RepoResult<Vec<Pipeline>>;

    async fn list_enabled_for_server(&self, server_id: ServerId) -> RepoResult<Vec<Pipeline>>;

    async fn update(&self, pipeline: &Pipeline) -> RepoResult<()>;

    async fn delete(&self, id: PipelineId) -> RepoResult<()>;

    async fn count_for_user(&self, server_id: ServerId, user_id: UserId) -> RepoResult<u64>;

    async fn set_enabled(&self, id: PipelineId, enabled: bool) -> RepoResult<()>;

    async fn set_last_fired_at(
        &self,
        id: PipelineId,
        fired_at: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<()>;
}
