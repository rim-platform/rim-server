use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::conversation::model::ConversationId;
use crate::incident::model::{
    Incident, IncidentId, IncidentMessageTag, IncidentPhaseName, IncidentPhaseId,
    IncidentPhaseRecord, IncidentStatus, NewIncident, NewIncidentMessageTag,
};
use crate::repository::error::RepoResult;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait IncidentRepository: Send + Sync {
    async fn create(&self, incident: NewIncident) -> RepoResult<Incident>;

    async fn find_by_id(&self, id: IncidentId) -> RepoResult<Option<Incident>>;

    async fn find_active_by_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> RepoResult<Option<Incident>>;

    async fn update_status(
        &self,
        id: IncidentId,
        status: IncidentStatus,
        mitigated_at: Option<chrono::DateTime<chrono::Utc>>,
        resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> RepoResult<Incident>;

    async fn update_postmortem_draft(
        &self,
        id: IncidentId,
        draft: String,
    ) -> RepoResult<Incident>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait IncidentPhaseRepository: Send + Sync {
    async fn create(
        &self,
        incident_id: IncidentId,
        phase: IncidentPhaseName,
        note: Option<String>,
    ) -> RepoResult<IncidentPhaseRecord>;

    async fn end_phase(
        &self,
        phase_id: IncidentPhaseId,
    ) -> RepoResult<IncidentPhaseRecord>;

    async fn find_current_phase(
        &self,
        incident_id: IncidentId,
    ) -> RepoResult<Option<IncidentPhaseRecord>>;

    async fn list_by_incident(
        &self,
        incident_id: IncidentId,
    ) -> RepoResult<Vec<IncidentPhaseRecord>>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait IncidentMessageTagRepository: Send + Sync {
    async fn create(&self, tag: NewIncidentMessageTag) -> RepoResult<IncidentMessageTag>;

    async fn list_by_incident(
        &self,
        incident_id: IncidentId,
    ) -> RepoResult<Vec<IncidentMessageTag>>;
}
