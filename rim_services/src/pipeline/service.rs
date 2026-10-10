use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::events::WorkspaceEvent;
use crate::pipeline::dto::{PipelineDto, PipelineFiredResult};
use crate::pipeline::request::{
    CreatePipelineCmd, DeletePipelineCmd, ListPipelinesCmd, TogglePipelineCmd, UpdatePipelineCmd,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait PipelineService: Send + Sync {
    async fn create(&self, cmd: CreatePipelineCmd) -> ServiceResult<PipelineDto>;

    async fn list(&self, cmd: ListPipelinesCmd) -> ServiceResult<Vec<PipelineDto>>;

    async fn update(&self, cmd: UpdatePipelineCmd) -> ServiceResult<PipelineDto>;

    async fn delete(&self, cmd: DeletePipelineCmd) -> ServiceResult<()>;

    async fn toggle(&self, cmd: TogglePipelineCmd) -> ServiceResult<PipelineDto>;

    async fn evaluate_event(
        &self,
        event: WorkspaceEvent,
    ) -> ServiceResult<Vec<PipelineFiredResult>>;
}
