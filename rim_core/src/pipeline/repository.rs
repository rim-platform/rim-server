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
    /// Insert `pipeline` and return the row as stored.
    async fn create(&self, pipeline: &Pipeline) -> RepoResult<Pipeline>;

    async fn get_by_id(&self, id: PipelineId) -> RepoResult<Option<Pipeline>>;

    async fn list_for_server(&self, server_id: ServerId) -> RepoResult<Vec<Pipeline>>;

    async fn list_enabled_for_server(&self, server_id: ServerId) -> RepoResult<Vec<Pipeline>>;

    /// Overwrite the definition of `pipeline` and return the row as stored.
    async fn update(&self, pipeline: &Pipeline) -> RepoResult<Pipeline>;

    async fn delete(&self, id: PipelineId) -> RepoResult<()>;

    async fn count_for_user(&self, server_id: ServerId, user_id: UserId) -> RepoResult<u64>;

    /// Switch the pipeline on or off and return the row as stored.
    async fn set_enabled(&self, id: PipelineId, enabled: bool) -> RepoResult<Pipeline>;

    async fn set_last_fired_at(
        &self,
        id: PipelineId,
        fired_at: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<()>;
}
