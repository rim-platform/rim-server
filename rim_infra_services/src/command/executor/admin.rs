use serde_json::Value;

use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};

use super::resolve_workspace_id;

pub(super) async fn invite_member(
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

    let result = services
        .auth_service
        .create_invite(rim_services::auth::command::CreateInviteCmd {
            actor,
            server_id,
            expires_at: None,
            max_uses: 1,
            recipient_email: None,
            role: rim_core::server_member::model::ServerMemberRole::Member,
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "invite created".to_string(),
        payload: Some(serde_json::json!({
            "invite_token": result.invite_token,
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::Actor;
    use rim_services::auth::dto::{CreateInviteResultDto, InviteDto};
    use rim_services::auth::service::MockAuthService;
    use rim_services::capability::service::MockCapabilityService;
    use rim_services::command::dto::{CommandContextDto, CommandResultStatusDto};
    use rim_services::conversation::service::MockConversationService;
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

    fn fake_invite_result(ws: ServerId) -> CreateInviteResultDto {
        CreateInviteResultDto {
            invite: InviteDto {
                id: rim_core::auth::model::InviteId::new(),
                server_id: ws,
                created_by_user_id: UserId::new(),
                created_at: chrono::Utc::now(),
                expires_at: None,
                max_uses: 1,
                use_count: 0,
                revoked_at: None,
                role: rim_core::server_member::model::ServerMemberRole::Member,
            },
            invite_token: "test-token".to_string(),
        }
    }

    fn make_services(auth: MockAuthService) -> CommandServices {
        CommandServices {
            auth_service: Arc::new(auth),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(MockPinService::new()),
            read_state_service: Arc::new(MockReadStateService::new()),
            server_member_service: Arc::new(MockServerMemberService::new()),
            user_service: Arc::new(MockUserService::new()),
            workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
            audit_log_service: None,
            moderation_service: std::sync::Arc::new(
                rim_services::moderation::service::MockModerationService::no_timeouts(),
            ),
            notification_service: std::sync::Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
        }
    }

    /// 1. AI path: args carry server_id — downstream sees arg UUID.
    #[tokio::test]
    async fn invite_member_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut auth = MockAuthService::new();
        auth.expect_create_invite()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_invite_result(ws)) }));

        let svc = make_services(auth);
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(ws.to_string()),
        );
        let result = super::invite_member(&svc, actor(), args, None)
            .await
            .expect("invite_member with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace.
    #[tokio::test]
    async fn invite_member_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut auth = MockAuthService::new();
        auth.expect_create_invite()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_invite_result(ws)) }));

        let svc = make_services(auth);
        let result =
            super::invite_member(&svc, actor(), serde_json::Map::new(), Some(ctx_with(ws)))
                .await
                .expect("invite_member with context workspace must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn invite_member_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut auth = MockAuthService::new();
        auth.expect_create_invite()
            .withf(move |cmd| cmd.server_id == arg_ws_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_invite_result(arg_ws)) }));

        let svc = make_services(auth);
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(arg_ws.to_string()),
        );
        let result = super::invite_member(&svc, actor(), args, Some(ctx_with(ctx_ws)))
            .await
            .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation error mentions both paths.
    #[tokio::test]
    async fn invite_member_both_absent_returns_validation() {
        let svc = make_services(MockAuthService::new());
        let err = super::invite_member(&svc, actor(), serde_json::Map::new(), None)
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
