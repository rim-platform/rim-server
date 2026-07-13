use std::sync::Arc;

use async_trait::async_trait;

use rim_core::conversation::repository::ConversationRepository;
use rim_core::escalation::model::{
    EscalationPolicyId, EscalationTarget, NewEscalationPolicy, NewEscalationStep,
};
use rim_core::escalation::repository::{
    ConversationEscalationPolicyRepository, EscalationPolicyRepository, EscalationStepRepository,
};
use rim_core::oncall::repository::OnCallScheduleRepository;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_services::ServiceError;
use rim_services::ServiceResult;
use rim_services::escalation::command::{
    CreateEscalationPolicyCmd, DeleteEscalationPolicyCmd, ListEscalationPoliciesQuery,
    SetChannelEscalationPolicyCmd, UpdateEscalationPolicyCmd,
};
use rim_services::escalation::dto::{
    EscalationPolicyDto, EscalationStepInput, escalation_policy_dto,
};
use rim_services::escalation::service::EscalationPolicyService;

use crate::util::{ensure_conversation_in_server, ensure_server_owner_or_moderator};

#[derive(Clone)]
pub struct EscalationPolicyRepos {
    pub escalation_policy_repo: Arc<dyn EscalationPolicyRepository>,
    pub escalation_step_repo: Arc<dyn EscalationStepRepository>,
    pub conversation_escalation_policy_repo: Arc<dyn ConversationEscalationPolicyRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    /// Used to validate an `OnCall(schedule_id)` step target actually exists before it's
    /// persisted (§3) — a dangling schedule id would otherwise silently page nobody at fire
    /// time instead of failing loudly at authoring time.
    pub on_call_schedule_repo: Arc<dyn OnCallScheduleRepository>,
}

pub struct DefaultEscalationPolicyService {
    repos: EscalationPolicyRepos,
}

impl DefaultEscalationPolicyService {
    pub fn new(repos: EscalationPolicyRepos) -> Self {
        Self { repos }
    }

    /// Validates a step input list against §3's invariants: at least one step, each with at
    /// least one action, and every target constrained to `server_id` — the policy's own server.
    /// This is the write-time half of the cross-tenant paging fix (C1 security review): without
    /// it, an Owner/Moderator of `server_id` could author a step that pages a `User` who is a
    /// member of a *different* server (or no server at all), or an `OnCall` schedule belonging
    /// to a different server — either of which would leak the incident title and channel link
    /// to that user at fire time (see `DefaultIncidentService::perform_step_actions`'s
    /// page-time recheck, the defense-in-depth half of the same fix). Position is assigned from
    /// array order by the caller (repository layer), not validated here.
    async fn validate_steps(
        &self,
        server_id: rim_core::server::model::ServerId,
        steps: &[EscalationStepInput],
    ) -> ServiceResult<()> {
        if steps.is_empty() {
            return Err(ServiceError::Validation(
                "escalation policy must have at least one step".to_string(),
            ));
        }
        for step in steps {
            if step.actions.is_empty() {
                return Err(ServiceError::Validation(
                    "each escalation step must have at least one action".to_string(),
                ));
            }
            if step.delay_seconds < 0 {
                return Err(ServiceError::Validation(
                    "escalation step delay_seconds must be >= 0".to_string(),
                ));
            }
            match step.target {
                EscalationTarget::User(user_id) => {
                    let is_member = self
                        .repos
                        .server_member_repo
                        .is_member(server_id, user_id)
                        .await?;
                    if !is_member {
                        return Err(ServiceError::Validation(format!(
                            "escalation step targets user {user_id} who is not a member of this server"
                        )));
                    }
                }
                EscalationTarget::OnCall(schedule_id) => {
                    let schedule = self
                        .repos
                        .on_call_schedule_repo
                        .find_by_id(schedule_id)
                        .await?;
                    match schedule {
                        Some(schedule) if schedule.server_id == server_id => {}
                        Some(_) => {
                            return Err(ServiceError::Validation(format!(
                                "escalation step targets on-call schedule {schedule_id} which \
                                 does not belong to this server"
                            )));
                        }
                        None => {
                            return Err(ServiceError::Validation(format!(
                                "escalation step targets unknown on-call schedule {schedule_id}"
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn to_new_steps(steps: Vec<EscalationStepInput>) -> Vec<NewEscalationStep> {
        steps
            .into_iter()
            .enumerate()
            .map(|(position, step)| NewEscalationStep {
                position: position as i32,
                delay_seconds: step.delay_seconds,
                target: step.target,
                actions: step.actions,
            })
            .collect()
    }

    async fn build_dto(&self, policy_id: EscalationPolicyId) -> ServiceResult<EscalationPolicyDto> {
        let policy = self
            .repos
            .escalation_policy_repo
            .find_by_id(policy_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let steps = self
            .repos
            .escalation_step_repo
            .list_by_policy(policy_id)
            .await?;
        Ok(escalation_policy_dto(policy, steps))
    }
}

#[async_trait]
impl EscalationPolicyService for DefaultEscalationPolicyService {
    async fn create_policy(
        &self,
        cmd: CreateEscalationPolicyCmd,
    ) -> ServiceResult<EscalationPolicyDto> {
        ensure_server_owner_or_moderator(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;
        self.validate_steps(cmd.server_id, &cmd.steps).await?;

        let policy = self
            .repos
            .escalation_policy_repo
            .create(NewEscalationPolicy {
                server_id: cmd.server_id,
                name: cmd.name,
                created_by: cmd.actor.user_id,
            })
            .await?;

        let steps = self
            .repos
            .escalation_step_repo
            .set_steps(policy.id, Self::to_new_steps(cmd.steps))
            .await?;

        Ok(escalation_policy_dto(policy, steps))
    }

    async fn update_policy(
        &self,
        cmd: UpdateEscalationPolicyCmd,
    ) -> ServiceResult<EscalationPolicyDto> {
        let policy = self
            .repos
            .escalation_policy_repo
            .find_by_id(cmd.policy_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        ensure_server_owner_or_moderator(
            self.repos.server_member_repo.as_ref(),
            policy.server_id,
            cmd.actor.user_id,
        )
        .await?;

        if let Some(steps) = &cmd.steps {
            self.validate_steps(policy.server_id, steps).await?;
        }

        if let Some(name) = cmd.name {
            self.repos
                .escalation_policy_repo
                .update_name(cmd.policy_id, name)
                .await?;
        }
        if let Some(steps) = cmd.steps {
            self.repos
                .escalation_step_repo
                .set_steps(cmd.policy_id, Self::to_new_steps(steps))
                .await?;
        }

        self.build_dto(cmd.policy_id).await
    }

    async fn delete_policy(&self, cmd: DeleteEscalationPolicyCmd) -> ServiceResult<()> {
        let policy = self
            .repos
            .escalation_policy_repo
            .find_by_id(cmd.policy_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        ensure_server_owner_or_moderator(
            self.repos.server_member_repo.as_ref(),
            policy.server_id,
            cmd.actor.user_id,
        )
        .await?;
        self.repos
            .escalation_policy_repo
            .delete(cmd.policy_id)
            .await?;
        Ok(())
    }

    async fn set_channel_policy(&self, cmd: SetChannelEscalationPolicyCmd) -> ServiceResult<()> {
        ensure_server_owner_or_moderator(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;
        ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;
        let policy = self
            .repos
            .escalation_policy_repo
            .find_by_id(cmd.policy_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if policy.server_id != cmd.server_id {
            return Err(ServiceError::Validation(
                "escalation policy does not belong to this server".to_string(),
            ));
        }

        self.repos
            .conversation_escalation_policy_repo
            .set(cmd.conversation_id, cmd.server_id, cmd.policy_id)
            .await?;
        Ok(())
    }

    async fn list_policies(
        &self,
        query: ListEscalationPoliciesQuery,
    ) -> ServiceResult<Vec<EscalationPolicyDto>> {
        crate::util::ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let policies = self
            .repos
            .escalation_policy_repo
            .list_by_server(query.server_id)
            .await?;
        let mut dtos = Vec::with_capacity(policies.len());
        for policy in policies {
            let steps = self
                .repos
                .escalation_step_repo
                .list_by_policy(policy.id)
                .await?;
            dtos.push(escalation_policy_dto(policy, steps));
        }
        Ok(dtos)
    }
}
