use serde_json::Value;

use rim_services::message::command::{DeleteMessageCmd, EditMessageCmd, SendMessageCmd};
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};

use super::{parse_uuid_arg, require_string_arg, resolve_workspace_id};

pub(super) async fn post_message(
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
    let text = require_string_arg(&args, "text")?;

    let message = services
        .message_service
        .send_message(SendMessageCmd {
            actor,
            server_id,
            conversation_id,
            text: Some(text),
            attachments: vec![],
            parent_message_id: None,
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "message posted".to_string(),
        payload: Some(serde_json::json!({
            "message_id": message.id.to_string(),
            "conversation_id": message.conversation_id.to_string(),
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

pub(super) async fn reply_to_selected_message(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let Some(context) = context else {
        return Err(ServiceError::Validation(
            "command requires context".to_string(),
        ));
    };
    let server_id = context.workspace_id.ok_or_else(|| {
        ServiceError::Validation("command requires workspace context".to_string())
    })?;
    let conversation_id = context.conversation_id.ok_or_else(|| {
        ServiceError::Validation("command requires conversation context".to_string())
    })?;
    let parent_message_id = context.selected_message_id.ok_or_else(|| {
        ServiceError::Validation("command requires selected message context".to_string())
    })?;
    let text = require_string_arg(&args, "text")?;

    let message = services
        .message_service
        .send_message(SendMessageCmd {
            actor,
            server_id,
            conversation_id,
            text: Some(text),
            attachments: vec![],
            parent_message_id: Some(parent_message_id),
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "reply posted".to_string(),
        payload: Some(serde_json::json!({
            "message_id": message.id.to_string(),
            "parent_message_id": parent_message_id.to_string(),
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

pub(super) async fn delete_message(
    services: &CommandServices,
    actor: Actor,
    _args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let Some(context) = context else {
        return Err(ServiceError::Validation(
            "command requires context".to_string(),
        ));
    };
    let server_id = context.workspace_id.ok_or_else(|| {
        ServiceError::Validation("command requires workspace context".to_string())
    })?;
    let conversation_id = context.conversation_id.ok_or_else(|| {
        ServiceError::Validation("command requires conversation context".to_string())
    })?;
    let message_id = context.selected_message_id.ok_or_else(|| {
        ServiceError::Validation("command requires selected message context".to_string())
    })?;

    services
        .message_service
        .delete_message(DeleteMessageCmd {
            actor,
            server_id,
            conversation_id,
            message_id,
            hard: false,
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "message deleted".to_string(),
        payload: Some(serde_json::json!({
            "message_id": message_id.to_string(),
            "conversation_id": conversation_id.to_string(),
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

pub(super) async fn edit_message(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let Some(context) = context else {
        return Err(ServiceError::Validation(
            "command requires context".to_string(),
        ));
    };
    let server_id = context.workspace_id.ok_or_else(|| {
        ServiceError::Validation("command requires workspace context".to_string())
    })?;
    let conversation_id = context.conversation_id.ok_or_else(|| {
        ServiceError::Validation("command requires conversation context".to_string())
    })?;
    let message_id = context.selected_message_id.ok_or_else(|| {
        ServiceError::Validation("command requires selected message context".to_string())
    })?;
    let text = require_string_arg(&args, "text")?;

    let message = services
        .message_service
        .edit_message(EditMessageCmd {
            actor,
            server_id,
            conversation_id,
            message_id,
            text,
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "message edited".to_string(),
        payload: Some(serde_json::json!({
            "message_id": message.id.to_string(),
            "conversation_id": message.conversation_id.to_string(),
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
    use rim_services::message::dto::MessageDto;
    use rim_services::message::service::MockMessageService;
    use rim_services::pin::service::MockPinService;
    use rim_services::read_state::service::MockReadStateService;
    use rim_services::server_member::service::MockServerMemberService;
    use rim_services::user::service::MockUserService;
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

    fn make_services(msg_svc: MockMessageService) -> CommandServices {
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(msg_svc),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(MockPinService::new()),
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
        }
    }

    fn fake_message(ws: ServerId, conv: ConversationId) -> MessageDto {
        MessageDto {
            id: MessageId::new(),
            server_id: ws,
            conversation_id: conv,
            thread_id: None,
            root_thread_id: None,
            author_id: UserId::new(),
            text: Some("hello".to_string()),
            has_attachments: false,
            attachments: vec![],
            parent_message_id: None,
            forwarded_from_message_id: None,
            thread_root_message_id: None,
            is_deleted: false,
            sent_at: chrono::Utc::now(),
        }
    }

    // ── post_message ─────────────────────────────────────────────────────────

    /// 1. AI path: args carry server_id — downstream sees arg UUID.
    #[tokio::test]
    async fn post_message_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let conv = ConversationId::new();
        let ws_check = ws;
        let conv_check = conv;
        let mut msg_svc = MockMessageService::new();
        msg_svc
            .expect_send_message()
            .withf(move |cmd| cmd.server_id == ws_check && cmd.conversation_id == conv_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_message(ws, conv)) }));

        let svc = make_services(msg_svc);
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
            "text".to_string(),
            serde_json::Value::String("hello".to_string()),
        );
        let result = super::post_message(&svc, actor(), args, None)
            .await
            .expect("post_message with arg server_id must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace.
    #[tokio::test]
    async fn post_message_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let conv = ConversationId::new();
        let ws_check = ws;
        let conv_check = conv;
        let mut msg_svc = MockMessageService::new();
        msg_svc
            .expect_send_message()
            .withf(move |cmd| cmd.server_id == ws_check && cmd.conversation_id == conv_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_message(ws, conv)) }));

        let svc = make_services(msg_svc);
        let mut args = serde_json::Map::new();
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(conv.to_string()),
        );
        args.insert(
            "text".to_string(),
            serde_json::Value::String("hello".to_string()),
        );
        let result = super::post_message(&svc, actor(), args, Some(ctx_with(ws)))
            .await
            .expect("post_message with context workspace must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn post_message_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        let conv = ConversationId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut msg_svc = MockMessageService::new();
        msg_svc
            .expect_send_message()
            .withf(move |cmd| cmd.server_id == arg_ws_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_message(arg_ws, conv)) }));

        let svc = make_services(msg_svc);
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(arg_ws.to_string()),
        );
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(conv.to_string()),
        );
        args.insert(
            "text".to_string(),
            serde_json::Value::String("hello".to_string()),
        );
        let result = super::post_message(&svc, actor(), args, Some(ctx_with(ctx_ws)))
            .await
            .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation error mentions both paths.
    #[tokio::test]
    async fn post_message_both_absent_returns_validation() {
        let conv = ConversationId::new();
        let svc = make_services(MockMessageService::new());
        let mut args = serde_json::Map::new();
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(conv.to_string()),
        );
        args.insert(
            "text".to_string(),
            serde_json::Value::String("hello".to_string()),
        );
        let err = super::post_message(&svc, actor(), args, None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, rim_services::ServiceError::Validation(_)),
            "expected Validation, got: {err:?}"
        );
        if let rim_services::ServiceError::Validation(msg) = err {
            assert!(msg.contains("server_id"));
            assert!(msg.contains("context.workspace_id"));
        }
    }
}
