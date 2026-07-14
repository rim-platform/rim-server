use serde_json::Value;

use rim_services::command::initiator::CommandInitiator;
use rim_services::incident::command::{AcknowledgeIncidentCmd, EscalateIncidentNowCmd};
use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};

use super::parse_uuid_arg;

/// `incident.ack` — calls the same `IncidentService::acknowledge_incident` method the
/// `acknowledgeIncident` GraphQL mutation calls (unified dispatch, §10.1). `initiator` is
/// threaded into `AcknowledgeIncidentCmd` (C2 §3b) so the service can append an
/// agent-transcript timeline note when the dispatch initiator is agentic.
pub(super) async fn ack(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    _context: Option<CommandContextDto>,
    initiator: CommandInitiator,
) -> ServiceResult<CommandResultDto> {
    let incident_id =
        parse_uuid_arg::<rim_core::incident::model::IncidentId>(&args, "incident_id")?;

    let incident = services
        .incident_service
        .acknowledge_incident(AcknowledgeIncidentCmd {
            actor,
            incident_id,
            initiator: Some(initiator),
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "incident acknowledged".to_string(),
        payload: Some(serde_json::json!({
            "success": true,
            "incident_id": incident.id.to_string(),
            "acknowledged_at": incident.acknowledged_at,
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

/// `incident.escalate` — calls the same `IncidentService::escalate_incident_now` method a
/// future `escalateIncidentNow` GraphQL mutation would call (unified dispatch, §10.1).
/// `initiator` is threaded into `EscalateIncidentNowCmd` (C2 §3b) so the service can append an
/// agent-transcript timeline note when the dispatch initiator is agentic.
pub(super) async fn escalate(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    _context: Option<CommandContextDto>,
    initiator: CommandInitiator,
) -> ServiceResult<CommandResultDto> {
    let incident_id =
        parse_uuid_arg::<rim_core::incident::model::IncidentId>(&args, "incident_id")?;

    services
        .incident_service
        .escalate_incident_now(EscalateIncidentNowCmd {
            actor,
            incident_id,
            initiator: Some(initiator),
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "escalation step triggered".to_string(),
        payload: Some(serde_json::json!({
            "success": true,
            "incident_id": incident_id.to_string(),
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::incident::model::{Incident, IncidentId, IncidentStatus};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::Actor;
    use rim_services::auth::service::MockAuthService;
    use rim_services::capability::service::MockCapabilityService;
    use rim_services::command::dto::CommandResultStatusDto;
    use rim_services::command::initiator::CommandInitiator;
    use rim_services::conversation::service::MockConversationService;
    use rim_services::incident::dto::IncidentDto;
    use rim_services::incident::service::MockIncidentService;
    use rim_services::message::service::MockMessageService;
    use rim_services::message_reaction::service::MockMessageReactionService;
    use rim_services::oncall::service::MockOnCallService;
    use rim_services::pin::service::MockPinService;
    use rim_services::read_state::service::MockReadStateService;
    use rim_services::server_member::service::MockServerMemberService;
    use rim_services::user::service::MockUserService;
    use rim_services::workspace_state::service::MockWorkspaceStateService;

    use crate::command::CommandServices;

    fn actor() -> Actor {
        Actor::new(UserId::new())
    }

    fn make_incident_dto(
        id: IncidentId,
        acknowledged_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> IncidentDto {
        let now = chrono::Utc::now();
        IncidentDto::from(Incident {
            id,
            server_id: ServerId::new(),
            conversation_id: rim_core::conversation::model::ConversationId::new(),
            started_by: UserId::new(),
            status: IncidentStatus::Active,
            severity: None,
            title: "t".to_string(),
            original_topic: None,
            started_at: now,
            mitigated_at: None,
            resolved_at: None,
            postmortem_draft: None,
            created_at: now,
            updated_at: now,
            alert_fingerprint: None,
            source: "manual".to_string(),
            escalation_policy_id: None,
            acknowledged_at,
            acknowledged_by: acknowledged_at.map(|_| UserId::new()),
        })
    }

    fn make_services(incident: MockIncidentService, oncall: MockOnCallService) -> CommandServices {
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(MockMessageReactionService::new()),
            pin_service: Arc::new(MockPinService::new()),
            read_state_service: Arc::new(MockReadStateService::new()),
            server_member_service: Arc::new(MockServerMemberService::new()),
            user_service: Arc::new(MockUserService::new()),
            workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
            audit_log_service: None,
            incident_service: Arc::new(incident),
            oncall_service: Arc::new(oncall),
        }
    }

    fn args_with(incident_id: IncidentId) -> serde_json::Map<String, serde_json::Value> {
        let mut args = serde_json::Map::new();
        args.insert(
            "incident_id".to_string(),
            serde_json::Value::String(incident_id.to_string()),
        );
        args
    }

    #[tokio::test]
    async fn ack_calls_acknowledge_incident_and_returns_success() {
        let incident_id = IncidentId::new();
        let now = chrono::Utc::now();
        let mut incident_service = MockIncidentService::new();
        incident_service
            .expect_acknowledge_incident()
            .withf(move |cmd| cmd.incident_id == incident_id)
            .return_once(move |_| {
                Box::pin(async move { Ok(make_incident_dto(incident_id, Some(now))) })
            });

        let svc = make_services(incident_service, MockOnCallService::new());
        let result = super::ack(
            &svc,
            actor(),
            args_with(incident_id),
            None,
            CommandInitiator::Palette,
        )
        .await
        .expect("ack must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    #[tokio::test]
    async fn ack_missing_incident_id_returns_validation() {
        let svc = make_services(MockIncidentService::new(), MockOnCallService::new());
        let err = super::ack(
            &svc,
            actor(),
            serde_json::Map::new(),
            None,
            CommandInitiator::Palette,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn escalate_calls_escalate_incident_now_and_returns_success() {
        let incident_id = IncidentId::new();
        let mut incident_service = MockIncidentService::new();
        incident_service
            .expect_escalate_incident_now()
            .withf(move |cmd| cmd.incident_id == incident_id)
            .return_once(|_| Box::pin(async { Ok(()) }));

        let svc = make_services(incident_service, MockOnCallService::new());
        let result = super::escalate(
            &svc,
            actor(),
            args_with(incident_id),
            None,
            CommandInitiator::Palette,
        )
        .await
        .expect("escalate must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    #[tokio::test]
    async fn escalate_missing_incident_id_returns_validation() {
        let svc = make_services(MockIncidentService::new(), MockOnCallService::new());
        let err = super::escalate(
            &svc,
            actor(),
            serde_json::Map::new(),
            None,
            CommandInitiator::Palette,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
    }

    // ── C2 §6.5: executor threading — initiator lands on the Cmd struct ──

    #[tokio::test]
    async fn ack_threads_agent_initiator_onto_acknowledge_incident_cmd() {
        let incident_id = IncidentId::new();
        let now = chrono::Utc::now();
        let mut incident_service = MockIncidentService::new();
        incident_service
            .expect_acknowledge_incident()
            .withf(|cmd| cmd.initiator == Some(CommandInitiator::Agent))
            .return_once(move |_| {
                Box::pin(async move { Ok(make_incident_dto(incident_id, Some(now))) })
            });

        let svc = make_services(incident_service, MockOnCallService::new());
        super::ack(
            &svc,
            actor(),
            args_with(incident_id),
            None,
            CommandInitiator::Agent,
        )
        .await
        .expect("ack must succeed");
    }

    #[tokio::test]
    async fn escalate_threads_agent_initiator_onto_escalate_incident_now_cmd() {
        let incident_id = IncidentId::new();
        let mut incident_service = MockIncidentService::new();
        incident_service
            .expect_escalate_incident_now()
            .withf(|cmd| cmd.initiator == Some(CommandInitiator::Agent))
            .return_once(|_| Box::pin(async { Ok(()) }));

        let svc = make_services(incident_service, MockOnCallService::new());
        super::escalate(
            &svc,
            actor(),
            args_with(incident_id),
            None,
            CommandInitiator::Agent,
        )
        .await
        .expect("escalate must succeed");
    }
}
