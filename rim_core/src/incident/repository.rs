use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::conversation::model::ConversationId;
use crate::incident::model::{
    Incident, IncidentId, IncidentMessageTag, IncidentPhaseId, IncidentPhaseName,
    IncidentPhaseRecord, IncidentSeverity, IncidentStatus, NewIncident, NewIncidentMessageTag,
};
use crate::repository::error::RepoResult;
use crate::repository::pagination::{Page, PageDir};
use crate::server::model::ServerId;

/// Cursor for paginating the incident history list.
///
/// Endpoint ordering: `created_at DESC, id DESC` (most recent first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncidentListCursor {
    pub created_at: DateTime<Utc>,
    pub incident_id: IncidentId,
}

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

    async fn update_postmortem_draft(&self, id: IncidentId, draft: String) -> RepoResult<Incident>;

    /// List incidents on a server, ordered by `created_at DESC, id DESC`.
    ///
    /// Filters within an array are OR-ed; the two arrays are AND-ed
    /// (e.g., `status IN (..) AND severity IN (..)`).
    /// `None` for a filter means "no filtering on that field".
    async fn list_incidents(
        &self,
        server_id: ServerId,
        status_filter: Option<&[IncidentStatus]>,
        severity_filter: Option<&[IncidentSeverity]>,
        limit: u32,
        dir: PageDir,
        cursor: Option<IncidentListCursor>,
    ) -> RepoResult<Page<Incident, IncidentListCursor>>;
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

    async fn end_phase(&self, phase_id: IncidentPhaseId) -> RepoResult<IncidentPhaseRecord>;

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
