use serde_json::Value;

use rim_services::message_reaction::command::ToggleReactionCmd;
use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};

use super::{parse_uuid_arg, reaction_has_actor, require_string_arg, resolve_workspace_id};

pub(super) async fn toggle_reaction(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    // P4-A5: name is `toggle_reaction` (Tony's call) — backend
    // mutation is bidirectional. `action` in the payload tells the
    // LLM whether the call added or removed the reaction, so a
    // subsequent turn doesn't blindly retry "add" when it meant to.
    // Detection: compare reaction groups pre/post for the actor's
    // user id + emoji. Simpler than reading the return type's
    // semantics directly since the toggle response is just the
    // final grouped state.
    let server_id = resolve_workspace_id::<rim_core::server::model::ServerId>(
        &args,
        context.as_ref(),
        "server_id",
    )?;
    let conversation_id =
        parse_uuid_arg::<rim_core::conversation::model::ConversationId>(&args, "conversation_id")?;
    let message_id = parse_uuid_arg::<rim_core::message::model::MessageId>(&args, "message_id")?;
    let emoji = require_string_arg(&args, "emoji")?;

    let actor_user_id = actor.user_id;

    let before = services
        .message_reaction_service
        .get_reactions(
            rim_services::message_reaction::command::GetMessageReactionsQuery {
                actor,
                server_id,
                conversation_id,
                message_id,
            },
        )
        .await?;
    let had_reaction = reaction_has_actor(&before, &emoji, actor_user_id);

    services
        .message_reaction_service
        .toggle_reaction(ToggleReactionCmd {
            actor,
            server_id,
            conversation_id,
            message_id,
            emoji: emoji.clone(),
        })
        .await?;

    let action = if had_reaction { "removed" } else { "added" };

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: format!("reaction {action}"),
        payload: Some(serde_json::json!({
            "success": true,
            "toggled": message_id.to_string(),
            "emoji": emoji,
            "action": action,
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
    use rim_services::message_reaction::dto::ReactionGroupDto;
    use rim_services::message_reaction::service::MockMessageReactionService;
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

    fn make_services(reaction: MockMessageReactionService) -> CommandServices {
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(reaction),
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

    fn reaction_args(
        ws: ServerId,
        conv: ConversationId,
        msg: MessageId,
        emoji: &str,
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
        args.insert(
            "emoji".to_string(),
            serde_json::Value::String(emoji.to_string()),
        );
        args
    }

    fn partial_reaction_args(
        conv: ConversationId,
        msg: MessageId,
        emoji: &str,
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
        args.insert(
            "emoji".to_string(),
            serde_json::Value::String(emoji.to_string()),
        );
        args
    }

    /// 1. AI path: args carry server_id — downstream sees arg UUID.
    #[tokio::test]
    async fn toggle_reaction_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let conv = ConversationId::new();
        let msg = MessageId::new();
        let ws_check = ws;
        let mut reaction = MockMessageReactionService::new();
        reaction
            .expect_get_reactions()
            .withf(move |q| q.server_id == ws_check)
            .return_once(|_| Box::pin(async { Ok(vec![]) }));
        reaction.expect_toggle_reaction().return_once(|_| {
            Box::pin(async {
                Ok(vec![ReactionGroupDto {
                    emoji: "👍".to_string(),
                    count: 1,
                    reacted_by_me: true,
                    user_ids: vec![],
                }])
            })
        });

        let svc = make_services(reaction);
        let result =
            super::toggle_reaction(&svc, actor(), reaction_args(ws, conv, msg, "👍"), None)
                .await
                .expect("toggle_reaction with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace.
    #[tokio::test]
    async fn toggle_reaction_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let conv = ConversationId::new();
        let msg = MessageId::new();
        let ws_check = ws;
        let mut reaction = MockMessageReactionService::new();
        reaction
            .expect_get_reactions()
            .withf(move |q| q.server_id == ws_check)
            .return_once(|_| Box::pin(async { Ok(vec![]) }));
        reaction
            .expect_toggle_reaction()
            .return_once(|_| Box::pin(async { Ok(vec![]) }));

        let svc = make_services(reaction);
        let result = super::toggle_reaction(
            &svc,
            actor(),
            partial_reaction_args(conv, msg, "👍"),
            Some(ctx_with(ws)),
        )
        .await
        .expect("toggle_reaction with context workspace must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn toggle_reaction_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        let conv = ConversationId::new();
        let msg = MessageId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut reaction = MockMessageReactionService::new();
        reaction
            .expect_get_reactions()
            .withf(move |q| q.server_id == arg_ws_check)
            .return_once(|_| Box::pin(async { Ok(vec![]) }));
        reaction
            .expect_toggle_reaction()
            .return_once(|_| Box::pin(async { Ok(vec![]) }));

        let svc = make_services(reaction);
        let result = super::toggle_reaction(
            &svc,
            actor(),
            reaction_args(arg_ws, conv, msg, "👍"),
            Some(ctx_with(ctx_ws)),
        )
        .await
        .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation error mentions both paths.
    #[tokio::test]
    async fn toggle_reaction_both_absent_returns_validation() {
        let conv = ConversationId::new();
        let msg = MessageId::new();
        let svc = make_services(MockMessageReactionService::new());
        let err =
            super::toggle_reaction(&svc, actor(), partial_reaction_args(conv, msg, "👍"), None)
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
