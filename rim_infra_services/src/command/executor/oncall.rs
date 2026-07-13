use serde_json::Value;

use rim_services::oncall::command::{CreateOnCallOverrideCmd, WhoIsOnCallQuery};
use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};

use super::{parse_rfc3339_arg, parse_uuid_arg};

/// `oncall.who` — calls the same `OnCallService::who_is_on_call` method the `whoIsOnCall`
/// GraphQL query calls (unified dispatch, §10.1). Always resolves "now" (`at: None`).
pub(super) async fn who(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    _context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let schedule_id =
        parse_uuid_arg::<rim_core::oncall::model::OnCallScheduleId>(&args, "schedule_id")?;

    let now = services
        .oncall_service
        .who_is_on_call(WhoIsOnCallQuery {
            actor,
            schedule_id,
            at: None,
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: match now.current_user_id {
            Some(user_id) => format!("{user_id} is on call"),
            None => "nobody is currently on call".to_string(),
        },
        payload: Some(serde_json::json!({
            "schedule_id": schedule_id.to_string(),
            "current_user_id": now.current_user_id.map(|u| u.to_string()),
            "shift_ends_at": now.shift_ends_at,
            "next_user_id": now.next_user_id.map(|u| u.to_string()),
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

/// `oncall.take_shift` — creates a self-service on-call override for the acting user, calling
/// the same `OnCallService::create_override` method `createOnCallOverride` calls (unified
/// dispatch, §10.1). The caller supplies `start_at`/`end_at` explicitly — this executor does not
/// invent a default "short" duration.
pub(super) async fn take_shift(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    _context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let schedule_id =
        parse_uuid_arg::<rim_core::oncall::model::OnCallScheduleId>(&args, "schedule_id")?;
    let start_at = parse_rfc3339_arg(&args, "start_at")?;
    let end_at = parse_rfc3339_arg(&args, "end_at")?;
    let actor_user_id = actor.user_id;

    let over = services
        .oncall_service
        .create_override(CreateOnCallOverrideCmd {
            actor,
            schedule_id,
            user_id: actor_user_id,
            start_at,
            end_at,
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "shift taken".to_string(),
        payload: Some(serde_json::json!({
            "success": true,
            "override_id": over.id.to_string(),
            "schedule_id": schedule_id.to_string(),
            "start_at": over.start_at,
            "end_at": over.end_at,
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::oncall::model::{OnCallOverrideId, OnCallScheduleId};
    use rim_core::user::model::UserId;
    use rim_services::Actor;
    use rim_services::auth::service::MockAuthService;
    use rim_services::capability::service::MockCapabilityService;
    use rim_services::command::dto::CommandResultStatusDto;
    use rim_services::conversation::service::MockConversationService;
    use rim_services::incident::service::MockIncidentService;
    use rim_services::message::service::MockMessageService;
    use rim_services::message_reaction::service::MockMessageReactionService;
    use rim_services::oncall::dto::{OnCallNowDto, OnCallOverrideDto};
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

    fn make_services(oncall: MockOnCallService) -> CommandServices {
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
            incident_service: Arc::new(MockIncidentService::new()),
            oncall_service: Arc::new(oncall),
        }
    }

    #[tokio::test]
    async fn who_returns_current_user_from_service() {
        let schedule_id = OnCallScheduleId::new();
        let on_call_user = UserId::new();
        let mut oncall = MockOnCallService::new();
        oncall
            .expect_who_is_on_call()
            .withf(move |q| q.schedule_id == schedule_id && q.at.is_none())
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(OnCallNowDto {
                        schedule_id,
                        current_user_id: Some(on_call_user),
                        shift_ends_at: None,
                        next_user_id: None,
                    })
                })
            });

        let mut args = serde_json::Map::new();
        args.insert(
            "schedule_id".to_string(),
            serde_json::Value::String(schedule_id.to_string()),
        );

        let svc = make_services(oncall);
        let result = super::who(&svc, actor(), args, None)
            .await
            .expect("who must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
        assert!(result.message.contains(&on_call_user.to_string()));
    }

    #[tokio::test]
    async fn who_missing_schedule_id_returns_validation() {
        let svc = make_services(MockOnCallService::new());
        let err = super::who(&svc, actor(), serde_json::Map::new(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn take_shift_creates_override_for_the_actor() {
        let schedule_id = OnCallScheduleId::new();
        let a = actor();
        let actor_user_id = a.user_id;
        let start = chrono::Utc::now();
        let end = start + chrono::Duration::hours(4);

        let mut oncall = MockOnCallService::new();
        oncall
            .expect_create_override()
            .withf(move |cmd| cmd.schedule_id == schedule_id && cmd.user_id == actor_user_id)
            .return_once(move |cmd| {
                Box::pin(async move {
                    Ok(OnCallOverrideDto {
                        id: OnCallOverrideId::new(),
                        schedule_id: cmd.schedule_id,
                        user_id: cmd.user_id,
                        start_at: cmd.start_at,
                        end_at: cmd.end_at,
                        created_by: cmd.actor.user_id,
                    })
                })
            });

        let mut args = serde_json::Map::new();
        args.insert(
            "schedule_id".to_string(),
            serde_json::Value::String(schedule_id.to_string()),
        );
        args.insert(
            "start_at".to_string(),
            serde_json::Value::String(start.to_rfc3339()),
        );
        args.insert(
            "end_at".to_string(),
            serde_json::Value::String(end.to_rfc3339()),
        );

        let svc = make_services(oncall);
        let result = super::take_shift(&svc, a, args, None)
            .await
            .expect("take_shift must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    #[tokio::test]
    async fn take_shift_missing_args_returns_validation() {
        let svc = make_services(MockOnCallService::new());
        let err = super::take_shift(&svc, actor(), serde_json::Map::new(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn take_shift_invalid_timestamp_returns_validation() {
        let mut args = serde_json::Map::new();
        args.insert(
            "schedule_id".to_string(),
            serde_json::Value::String(OnCallScheduleId::new().to_string()),
        );
        args.insert(
            "start_at".to_string(),
            serde_json::Value::String("not-a-timestamp".to_string()),
        );
        args.insert(
            "end_at".to_string(),
            serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
        );

        let svc = make_services(MockOnCallService::new());
        let err = super::take_shift(&svc, actor(), args, None)
            .await
            .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
    }
}
