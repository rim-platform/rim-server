use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::escalation::command::{
    CreateEscalationPolicyCmd, DeleteEscalationPolicyCmd, ListEscalationPoliciesQuery,
    SetChannelEscalationPolicyCmd, UpdateEscalationPolicyCmd,
};
use crate::escalation::dto::EscalationPolicyDto;

/// CRUD for escalation policies + their channel binding (§3). Arming/executing a run against a
/// *declared incident* is owned by `IncidentService` (see `docs/oncall.md` "Escalation wiring")
/// — this trait is purely the policy-authoring surface.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait EscalationPolicyService: Send + Sync {
    async fn create_policy(
        &self,
        cmd: CreateEscalationPolicyCmd,
    ) -> ServiceResult<EscalationPolicyDto>;

    async fn update_policy(
        &self,
        cmd: UpdateEscalationPolicyCmd,
    ) -> ServiceResult<EscalationPolicyDto>;

    async fn delete_policy(&self, cmd: DeleteEscalationPolicyCmd) -> ServiceResult<()>;

    async fn set_channel_policy(&self, cmd: SetChannelEscalationPolicyCmd) -> ServiceResult<()>;

    async fn list_policies(
        &self,
        query: ListEscalationPoliciesQuery,
    ) -> ServiceResult<Vec<EscalationPolicyDto>>;
}
