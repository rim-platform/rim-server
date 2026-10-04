#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;

    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::escalation::model::{
        EscalationAction, EscalationPolicy, EscalationPolicyId, EscalationStep, EscalationStepId,
        EscalationTarget,
    };
    use rim_core::escalation::repository::{
        MockConversationEscalationPolicyRepository, MockEscalationPolicyRepository,
        MockEscalationStepRepository,
    };
    use rim_core::oncall::model::{OnCallSchedule, OnCallScheduleId};
    use rim_core::oncall::repository::MockOnCallScheduleRepository;
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_services::escalation::command::{CreateEscalationPolicyCmd, UpdateEscalationPolicyCmd};
    use rim_services::escalation::dto::EscalationStepInput;
    use rim_services::escalation::service::EscalationPolicyService;
    use rim_services::{Actor, ServiceError};

    use crate::escalation::{DefaultEscalationPolicyService, EscalationPolicyRepos};

    fn owner_member(server_id: ServerId, user_id: UserId) -> ServerMember {
        ServerMember {
            id: ServerMemberId::new(),
            server_id,
            user_id,
            member_role: ServerMemberRole::Owner,
            joined_at: Utc::now(),
        }
    }

    fn make_policy(server_id: ServerId, created_by: UserId) -> EscalationPolicy {
        let now = Utc::now();
        EscalationPolicy {
            id: EscalationPolicyId::new(),
            server_id,
            name: "Primary".to_string(),
            created_by,
            created_at: now,
            updated_at: now,
        }
    }

    fn make_schedule(server_id: ServerId) -> OnCallSchedule {
        let now = Utc::now();
        OnCallSchedule {
            id: OnCallScheduleId::new(),
            server_id,
            name: "Primary".to_string(),
            timezone: "UTC".to_string(),
            created_by: UserId::new(),
            created_at: now,
            updated_at: now,
        }
    }

    fn one_step(target: EscalationTarget) -> Vec<EscalationStepInput> {
        vec![EscalationStepInput {
            delay_seconds: 0,
            target,
            actions: vec![EscalationAction::Email],
        }]
    }

    fn build_service(
        escalation_policy_repo: MockEscalationPolicyRepository,
        escalation_step_repo: MockEscalationStepRepository,
        server_member_repo: MockServerMemberRepository,
        on_call_schedule_repo: MockOnCallScheduleRepository,
    ) -> DefaultEscalationPolicyService {
        DefaultEscalationPolicyService::new(EscalationPolicyRepos {
            escalation_policy_repo: Arc::new(escalation_policy_repo),
            escalation_step_repo: Arc::new(escalation_step_repo),
            conversation_escalation_policy_repo: Arc::new(
                MockConversationEscalationPolicyRepository::new(),
            ),
            server_member_repo: Arc::new(server_member_repo),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            on_call_schedule_repo: Arc::new(on_call_schedule_repo),
        })
    }

    // ── create_policy: HIGH — cross-tenant target rejection (C1 security review) ───────────

    #[tokio::test]
    async fn create_policy_rejects_user_target_not_a_member_of_this_server() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let outsider_id = UserId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));
        // The target user is not a member of `server_id` — e.g. a member of a different server,
        // or no server at all.
        server_member_repo
            .expect_is_member()
            .withf(move |sid, uid| *sid == server_id && *uid == outsider_id)
            .returning(|_, _| Ok(false));

        // No escalation_policy_repo/escalation_step_repo expectations set — a rejected
        // validation must never reach the create/set_steps writes (zero-expectation-is-the-
        // assertion style used throughout this codebase).
        let svc = build_service(
            MockEscalationPolicyRepository::new(),
            MockEscalationStepRepository::new(),
            server_member_repo,
            MockOnCallScheduleRepository::new(),
        );

        let err = svc
            .create_policy(CreateEscalationPolicyCmd {
                actor: Actor::new(actor_id),
                server_id,
                name: "Primary".to_string(),
                steps: one_step(EscalationTarget::User(outsider_id)),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_policy_rejects_on_call_schedule_from_another_server() {
        let server_id = ServerId::new();
        let other_server_id = ServerId::new();
        let actor_id = UserId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));

        // The schedule exists, but belongs to a *different* server than the policy being
        // authored — must be rejected even though the schedule itself is real.
        let foreign_schedule = make_schedule(other_server_id);
        let schedule_id = foreign_schedule.id;
        let mut on_call_schedule_repo = MockOnCallScheduleRepository::new();
        on_call_schedule_repo
            .expect_find_by_id()
            .returning(move |_| {
                let s = foreign_schedule.clone();
                Box::pin(async move { Ok(Some(s)) })
            });

        let svc = build_service(
            MockEscalationPolicyRepository::new(),
            MockEscalationStepRepository::new(),
            server_member_repo,
            on_call_schedule_repo,
        );

        let err = svc
            .create_policy(CreateEscalationPolicyCmd {
                actor: Actor::new(actor_id),
                server_id,
                name: "Primary".to_string(),
                steps: one_step(EscalationTarget::OnCall(schedule_id)),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_policy_accepts_same_server_member_and_schedule() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let member_id = UserId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let policy = make_policy(server_id, actor_id);
        let policy_for_create = policy.clone();
        let mut escalation_policy_repo = MockEscalationPolicyRepository::new();
        escalation_policy_repo.expect_create().returning(move |_| {
            let p = policy_for_create.clone();
            Box::pin(async move { Ok(p) })
        });

        let policy_id = policy.id;
        let mut escalation_step_repo = MockEscalationStepRepository::new();
        escalation_step_repo
            .expect_set_steps()
            .returning(move |_, new_steps| {
                let policy_id = policy_id;
                let steps: Vec<EscalationStep> = new_steps
                    .into_iter()
                    .enumerate()
                    .map(|(i, s)| EscalationStep {
                        id: EscalationStepId::new(),
                        policy_id,
                        position: i as i32,
                        delay_seconds: s.delay_seconds,
                        target: s.target,
                        actions: s.actions,
                    })
                    .collect();
                Box::pin(async move { Ok(steps) })
            });

        let svc = build_service(
            escalation_policy_repo,
            escalation_step_repo,
            server_member_repo,
            MockOnCallScheduleRepository::new(),
        );

        let dto = svc
            .create_policy(CreateEscalationPolicyCmd {
                actor: Actor::new(actor_id),
                server_id,
                name: "Primary".to_string(),
                steps: one_step(EscalationTarget::User(member_id)),
            })
            .await
            .unwrap();
        assert_eq!(dto.steps.len(), 1);
    }

    // ── update_policy: same fix must also apply on the update path ─────────────────────────

    #[tokio::test]
    async fn update_policy_rejects_user_target_not_a_member_of_this_server() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let outsider_id = UserId::new();
        let policy = make_policy(server_id, actor_id);
        let policy_id = policy.id;

        let mut escalation_policy_repo = MockEscalationPolicyRepository::new();
        escalation_policy_repo
            .expect_find_by_id()
            .returning(move |_| {
                let p = policy.clone();
                Box::pin(async move { Ok(Some(p)) })
            });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(owner_member(server_id, actor_id))));
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(false));

        // No set_steps/update_name expectations — rejected before any write.
        let svc = build_service(
            escalation_policy_repo,
            MockEscalationStepRepository::new(),
            server_member_repo,
            MockOnCallScheduleRepository::new(),
        );

        let err = svc
            .update_policy(UpdateEscalationPolicyCmd {
                actor: Actor::new(actor_id),
                policy_id,
                name: None,
                steps: Some(one_step(EscalationTarget::User(outsider_id))),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }
}
