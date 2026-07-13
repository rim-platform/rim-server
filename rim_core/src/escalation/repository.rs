use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::conversation::model::ConversationId;
use crate::escalation::model::{
    EscalationPolicy, EscalationPolicyId, EscalationRun, EscalationRunId, EscalationRunStatus,
    EscalationStep, NewEscalationPolicy, NewEscalationRun, NewEscalationStep,
};
use crate::incident::model::IncidentId;
use crate::repository::error::RepoResult;
use crate::server::model::ServerId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait EscalationPolicyRepository: Send + Sync {
    async fn create(&self, new: NewEscalationPolicy) -> RepoResult<EscalationPolicy>;

    async fn find_by_id(&self, id: EscalationPolicyId) -> RepoResult<Option<EscalationPolicy>>;

    /// List all policies on a server, ordered by `created_at ASC, id ASC`.
    async fn list_by_server(&self, server_id: ServerId) -> RepoResult<Vec<EscalationPolicy>>;

    async fn update_name(
        &self,
        id: EscalationPolicyId,
        name: String,
    ) -> RepoResult<EscalationPolicy>;

    /// Deletes the policy and (via FK cascade) its steps and any
    /// `conversation_escalation_policy` bindings.
    async fn delete(&self, id: EscalationPolicyId) -> RepoResult<()>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait EscalationStepRepository: Send + Sync {
    /// List a policy's steps, ordered by `position ASC`.
    async fn list_by_policy(
        &self,
        policy_id: EscalationPolicyId,
    ) -> RepoResult<Vec<EscalationStep>>;

    /// Replace the full ordered step list for a policy (delete-then-insert), mirroring
    /// [`crate::oncall::repository::RotationParticipantRepository::set_participants`].
    /// `steps[0]` becomes `position = 0`, and so on.
    async fn set_steps(
        &self,
        policy_id: EscalationPolicyId,
        steps: Vec<NewEscalationStep>,
    ) -> RepoResult<Vec<EscalationStep>>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ConversationEscalationPolicyRepository: Send + Sync {
    /// Upserts the default escalation policy for `conversation_id`.
    async fn set(
        &self,
        conversation_id: ConversationId,
        server_id: ServerId,
        policy_id: EscalationPolicyId,
    ) -> RepoResult<()>;

    async fn get(&self, conversation_id: ConversationId) -> RepoResult<Option<EscalationPolicyId>>;
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait EscalationRunRepository: Send + Sync {
    async fn create(&self, new: NewEscalationRun) -> RepoResult<EscalationRun>;

    async fn find_by_id(&self, id: EscalationRunId) -> RepoResult<Option<EscalationRun>>;

    /// The `status = 'active'` run for an incident, if any — enforced unique by the partial
    /// unique index `(incident_id) WHERE status='active'`.
    async fn find_active_by_incident(
        &self,
        incident_id: IncidentId,
    ) -> RepoResult<Option<EscalationRun>>;

    /// Every `status = 'active'` run, for boot-recovery re-arming. Backed by the partial index
    /// `(status, next_fire_at) WHERE status='active'`.
    async fn list_active(&self) -> RepoResult<Vec<EscalationRun>>;

    /// Advance to the next step (or leave `next_fire_at: None` when there is no next step —
    /// callers should pair that with [`Self::set_status`] to `Completed`).
    async fn advance(
        &self,
        id: EscalationRunId,
        current_step_index: i32,
        next_fire_at: Option<DateTime<Utc>>,
    ) -> RepoResult<EscalationRun>;

    /// Transition the run's status (e.g. to `Cancelled` on ack, or `Completed` after the last
    /// step fires). Clears `next_fire_at` when moving out of `Active`.
    async fn set_status(
        &self,
        id: EscalationRunId,
        status: EscalationRunStatus,
    ) -> RepoResult<EscalationRun>;
}
