use serde_json::Value;

use rim_services::pin::command::PinMessageCmd;
use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};

use super::{parse_uuid_arg, resolve_workspace_id};

pub(super) async fn pin_message(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id::<rim_core::server::model::ServerId>(
        &args,
        context.as_ref(),
        "server_id",
    )?;
    let conversation_id =
        parse_uuid_arg::<rim_core::conversation::model::ConversationId>(&args, "conversation_id")?;
    let message_id = parse_uuid_arg::<rim_core::message::model::MessageId>(&args, "message_id")?;

    services
        .pin_service
        .pin_message(PinMessageCmd {
            actor,
            server_id,
            conversation_id,
            message_id,
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "message pinned".to_string(),
        payload: Some(serde_json::json!({
            "success": true,
            "pinned": message_id.to_string(),
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::model::ConversationId;
    use rim_core::message::model::MessageId;
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::Actor;
    use rim_services::auth::service::MockAuthService;
    use rim_services::capability::service::MockCapabilityService;
    use rim_services::command::dto::{CommandContextDto, CommandResultStatusDto};
    use rim_services::conversation::service::MockConversationService;
    use rim_services::message::service::MockMessageService;
    use rim_services::pin::service::MockPinService;
    use rim_services::read_state::service::MockReadStateService;
    use rim_services::server_member::service::MockServerMemberService;
    use rim_services::user::service::MockUserService;
    use rim_services::workspace_feature::service::MockWorkspaceFeatureService;
    use rim_services::workspace_state::service::MockWorkspaceStateService;

    use crate::command::CommandServices;

    fn actor() -> Actor {
        Actor::new(UserId::new())
    }

    fn ctx_with(ws: ServerId) -> CommandContextDto {
        CommandContextDto {
            workspace_id: Some(ws),
            conversation_id: None,
            thread_id: None,
            selected_text: None,
            selected_message_id: None,
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        }
    }

    fn make_services(pin: MockPinService) -> CommandServices {
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(pin),
            read_state_service: Arc::new(MockReadStateService::new()),
            server_member_service: Arc::new(MockServerMemberService::new()),
            user_service: Arc::new(MockUserService::new()),
            workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
            audit_log_service: None,
            incident_service: std::sync::Arc::new(
                rim_services::incident::service::MockIncidentService::new(),
            ),
            oncall_service: std::sync::Arc::new(
                rim_services::oncall::service::MockOnCallService::new(),
            ),
            workspace_feature_service: std::sync::Arc::new(
                MockWorkspaceFeatureService::all_enabled(),
            ),
            moderation_service: std::sync::Arc::new(
                rim_services::moderation::service::MockModerationService::no_timeouts(),
            ),
        }
    }

    fn full_args(
        ws: ServerId,
        conv: ConversationId,
        msg: MessageId,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(ws.to_string()),
        );
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(conv.to_string()),
        );
        args.insert(
            "message_id".to_string(),
            serde_json::Value::String(msg.to_string()),
        );
        args
    }

    fn partial_args(
        conv: ConversationId,
        msg: MessageId,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut args = serde_json::Map::new();
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(conv.to_string()),
        );
        args.insert(
            "message_id".to_string(),
            serde_json::Value::String(msg.to_string()),
        );
        args
    }

    /// 1. AI path: args carry server_id — downstream sees arg UUID.
    #[tokio::test]
    async fn pin_message_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let conv = ConversationId::new();
        let msg = MessageId::new();
        let ws_check = ws;
        let mut pin = MockPinService::new();
        pin.expect_pin_message()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(|_| Box::pin(async { Ok(()) }));

        let svc = make_services(pin);
        let result = super::pin_message(&svc, actor(), full_args(ws, conv, msg), None)
            .await
            .expect("pin_message with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace.
    #[tokio::test]
    async fn pin_message_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let conv = ConversationId::new();
        let msg = MessageId::new();
        let ws_check = ws;
        let mut pin = MockPinService::new();
        pin.expect_pin_message()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(|_| Box::pin(async { Ok(()) }));

        let svc = make_services(pin);
        let result = super::pin_message(&svc, actor(), partial_args(conv, msg), Some(ctx_with(ws)))
            .await
            .expect("pin_message with context workspace must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn pin_message_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        let conv = ConversationId::new();
        let msg = MessageId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut pin = MockPinService::new();
        pin.expect_pin_message()
            .withf(move |cmd| cmd.server_id == arg_ws_check)
            .return_once(|_| Box::pin(async { Ok(()) }));

        let svc = make_services(pin);
        let result = super::pin_message(
            &svc,
            actor(),
            full_args(arg_ws, conv, msg),
            Some(ctx_with(ctx_ws)),
        )
        .await
        .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation error mentions both paths.
    #[tokio::test]
    async fn pin_message_both_absent_returns_validation() {
        let conv = ConversationId::new();
        let msg = MessageId::new();
        let svc = make_services(MockPinService::new());
        let err = super::pin_message(&svc, actor(), partial_args(conv, msg), None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, rim_services::ServiceError::Validation(_)),
            "expected Validation, got: {err:?}"
        );
        if let rim_services::ServiceError::Validation(msg_text) = err {
            assert!(msg_text.contains("server_id"));
            assert!(msg_text.contains("context.workspace_id"));
        }
    }
}
