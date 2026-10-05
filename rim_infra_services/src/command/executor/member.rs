use serde_json::Value;

use rim_services::server_member::command::ListServerMembersQuery;
use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};

use super::resolve_workspace_id;

pub(super) async fn list_members(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    // P4-A5: thin adapter over ServerMemberService::list_members.
    // Projects to `{members: [{username, role, joined}]}` ONLY.
    // Widow no-go: do NOT leak `email`, `user_id`, `avatar_url`,
    // `display_name`, `bio`, `status_text`. A future regression
    // that passes the ServerMemberDto through serde directly
    // would re-widen PII — the explicit map below is the gate.
    let server_id = resolve_workspace_id::<rim_core::server::model::ServerId>(
        &args,
        context.as_ref(),
        "server_id",
    )?;

    let members = services
        .server_member_service
        .list_members(ListServerMembersQuery { actor, server_id })
        .await?;

    let projected: Vec<Value> = members
        .iter()
        .map(|m| {
            serde_json::json!({
                "username": m.username,
                "role": m.member_role.as_str(),
                "joined": m.joined_at.to_rfc3339(),
            })
        })
        .collect();

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: format!("{} members found", projected.len()),
        payload: Some(serde_json::json!({ "members": projected })),
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
    use rim_services::auth::service::MockAuthService;
    use rim_services::capability::service::MockCapabilityService;
    use rim_services::command::dto::{CommandContextDto, CommandResultStatusDto};
    use rim_services::conversation::service::MockConversationService;
    use rim_services::message::service::MockMessageService;
    use rim_services::pin::service::MockPinService;
    use rim_services::read_state::service::MockReadStateService;
    use rim_services::server_member::dto::ServerMemberDto;
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

    fn make_services(sm: MockServerMemberService) -> CommandServices {
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(MockPinService::new()),
            read_state_service: Arc::new(MockReadStateService::new()),
            server_member_service: Arc::new(sm),
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
            notification_service: std::sync::Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
        }
    }

    fn fake_member(ws: ServerId) -> ServerMemberDto {
        ServerMemberDto {
            server_id: ws,
            user_id: UserId::new(),
            username: "alice".to_string(),
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
            member_role: rim_core::server_member::model::ServerMemberRole::Member,
            joined_at: chrono::Utc::now(),
            timeout_until: None,
        }
    }

    /// 1. AI path: args carry server_id — downstream sees arg UUID.
    #[tokio::test]
    async fn list_members_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut sm = MockServerMemberService::new();
        sm.expect_list_members()
            .withf(move |q| q.server_id == ws_check)
            .return_once(move |_| Box::pin(async move { Ok(vec![fake_member(ws)]) }));

        let svc = make_services(sm);
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(ws.to_string()),
        );
        let result = super::list_members(&svc, actor(), args, None)
            .await
            .expect("list_members with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace.
    #[tokio::test]
    async fn list_members_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut sm = MockServerMemberService::new();
        sm.expect_list_members()
            .withf(move |q| q.server_id == ws_check)
            .return_once(move |_| Box::pin(async move { Ok(vec![fake_member(ws)]) }));

        let svc = make_services(sm);
        let result = super::list_members(&svc, actor(), serde_json::Map::new(), Some(ctx_with(ws)))
            .await
            .expect("list_members with context workspace must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn list_members_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut sm = MockServerMemberService::new();
        sm.expect_list_members()
            .withf(move |q| q.server_id == arg_ws_check)
            .return_once(move |_| Box::pin(async move { Ok(vec![]) }));

        let svc = make_services(sm);
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(arg_ws.to_string()),
        );
        let result = super::list_members(&svc, actor(), args, Some(ctx_with(ctx_ws)))
            .await
            .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation error mentions both paths.
    #[tokio::test]
    async fn list_members_both_absent_returns_validation() {
        let svc = make_services(MockServerMemberService::new());
        let err = super::list_members(&svc, actor(), serde_json::Map::new(), None)
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
