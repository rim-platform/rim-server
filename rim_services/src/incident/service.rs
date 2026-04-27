use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::repository::pagination::Page;

use crate::ServiceResult;
use crate::incident::command::{
    AdvancePhaseCmd, GeneratePostmortemCmd, GetActiveIncidentQuery, GetIncidentTimelineQuery,
    ListIncidentsQuery, ResolveIncidentCmd, StartIncidentCmd, TagMessageCmd,
    UpdateIncidentStatusCmd,
};
use crate::incident::dto::{
    IncidentDto, IncidentListPageCursor, IncidentMessageTagDto, IncidentPhaseDto,
    IncidentTimelineDto,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait IncidentService: Send + Sync {
    async fn start_incident(&self, cmd: StartIncidentCmd) -> ServiceResult<IncidentDto>;

    async fn update_incident_status(
        &self,
        cmd: UpdateIncidentStatusCmd,
    ) -> ServiceResult<IncidentDto>;

    async fn advance_phase(&self, cmd: AdvancePhaseCmd) -> ServiceResult<IncidentPhaseDto>;

    async fn tag_message(&self, cmd: TagMessageCmd) -> ServiceResult<IncidentMessageTagDto>;

    async fn generate_postmortem(&self, cmd: GeneratePostmortemCmd) -> ServiceResult<IncidentDto>;

    async fn resolve_incident(&self, cmd: ResolveIncidentCmd) -> ServiceResult<IncidentDto>;

    async fn get_active_incident(
        &self,
        query: GetActiveIncidentQuery,
    ) -> ServiceResult<Option<IncidentDto>>;

    async fn get_incident_timeline(
        &self,
        query: GetIncidentTimelineQuery,
    ) -> ServiceResult<IncidentTimelineDto>;

    async fn list_incidents(
        &self,
        query: ListIncidentsQuery,
    ) -> ServiceResult<Page<IncidentDto, IncidentListPageCursor>>;
}
