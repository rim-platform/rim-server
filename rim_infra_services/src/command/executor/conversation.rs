use serde_json::Value;

use rim_services::conversation::command::{
    AddConversationMembersCmd, ArchiveConversationCmd, CreateConversationCmd, JoinChannelCmd,
    LeaveConversationCmd, RemoveConversationMemberCmd,
};
use rim_services::conversation::types::ConversationParticipants;
use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};

use super::{
    optional_string_arg, parse_uuid_arg, parse_uuid_array_arg, resolve_conversation_id,
    resolve_workspace_id,
};

fn success(message: &str, payload: serde_json::Value) -> CommandResultDto {
    CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: message.to_string(),
        payload: Some(payload),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    }
}

/// The `(server_id, conversation_id)` pair every membership command targets, each from its arg
/// or, when omitted, from the focused workspace/conversation in `context`.
fn resolve_target(
    args: &serde_json::Map<String, Value>,
    context: Option<&CommandContextDto>,
) -> ServiceResult<(
    rim_core::server::model::ServerId,
    rim_core::conversation::model::ConversationId,
)> {
    Ok((
        resolve_workspace_id(args, context, "server_id")?,
        resolve_conversation_id(args, context, "conversation_id")?,
    ))
}

pub(super) async fn join_channel(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let (server_id, conversation_id) = resolve_target(&args, context.as_ref())?;
    let conversation = services
        .conversation_service
        .join_channel(JoinChannelCmd {
            actor,
            server_id,
            conversation_id,
        })
        .await?;
    Ok(success(
        "joined channel",
        serde_json::json!({ "conversation_id": conversation.id.to_string() }),
    ))
}

pub(super) async fn leave_conversation(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let (server_id, conversation_id) = resolve_target(&args, context.as_ref())?;
    services
        .conversation_service
        .leave_conversation(LeaveConversationCmd {
            actor,
            server_id,
            conversation_id,
        })
        .await?;
    Ok(success(
        "left conversation",
        serde_json::json!({ "conversation_id": conversation_id.to_string() }),
    ))
}

pub(super) async fn invite_to_conversation(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let (server_id, conversation_id) = resolve_target(&args, context.as_ref())?;
    let user_ids = parse_uuid_array_arg::<rim_core::user::model::UserId>(&args, "user_ids")?;
    let conversation = services
        .conversation_service
        .add_conversation_members(AddConversationMembersCmd {
            actor,
            server_id,
            conversation_id,
            user_ids,
        })
        .await?;
    Ok(success(
        "members added",
        serde_json::json!({
            "conversation_id": conversation.id.to_string(),
            "member_count": conversation.member_count(),
        }),
    ))
}

pub(super) async fn set_channel_archived(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
    archived: bool,
) -> ServiceResult<CommandResultDto> {
    let (server_id, conversation_id) = resolve_target(&args, context.as_ref())?;
    let conversation = services
        .conversation_service
        .set_conversation_archived(ArchiveConversationCmd {
            actor,
            server_id,
            conversation_id,
            archived,
        })
        .await?;
    Ok(success(
        if archived {
            "channel archived"
        } else {
            "channel unarchived"
        },
        serde_json::json!({
            "conversation_id": conversation.id.to_string(),
            "archived": conversation.is_archived(),
        }),
    ))
}

pub(super) async fn remove_conversation_member(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let (server_id, conversation_id) = resolve_target(&args, context.as_ref())?;
    let user_id = parse_uuid_arg::<rim_core::user::model::UserId>(&args, "user_id")?;
    services
        .conversation_service
        .remove_conversation_member(RemoveConversationMemberCmd {
            actor,
            server_id,
            conversation_id,
            user_id,
        })
        .await?;
    Ok(success(
        "member removed",
        serde_json::json!({
            "conversation_id": conversation_id.to_string(),
            "user_id": user_id.to_string(),
        }),
    ))
}

pub(super) async fn create_group(
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
    let member_ids = parse_uuid_array_arg::<rim_core::user::model::UserId>(&args, "member_ids")?;
    let conversation = services
        .conversation_service
        .create_conversation(CreateConversationCmd {
            actor,
            server_id,
            participants: ConversationParticipants::Group { member_ids },
            title: optional_string_arg(&args, "title"),
            is_private: false,
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "group created".to_string(),
        payload: Some(serde_json::json!({
            "conversation_id": conversation.id.to_string(),
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::model::{ConversationId, ConversationType};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::Actor;
    use rim_services::auth::service::MockAuthService;
    use rim_services::capability::service::MockCapabilityService;
    use rim_services::command::dto::{CommandContextDto, CommandResultStatusDto};
    use rim_services::conversation::dto::ConversationDto;
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

    fn make_services(conv: MockConversationService) -> CommandServices {
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(conv),
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

    fn fake_group(ws: ServerId) -> ConversationDto {
        ConversationDto {
            id: ConversationId::new(),
            server_id: ws,
            conversation_type: ConversationType::Group,
            title: None,
            topic: None,
            member_ids: vec![],
            is_private: false,
            archived_at: None,
            created_by: None,
            slow_mode_seconds: 0,
        }
    }

    fn member_ids_arg(uids: &[UserId]) -> serde_json::Value {
        serde_json::Value::Array(
            uids.iter()
                .map(|id| serde_json::Value::String(id.to_string()))
                .collect(),
        )
    }

    /// 1. AI path: args carry server_id — downstream sees arg UUID.
    #[tokio::test]
    async fn create_group_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let members = vec![UserId::new()];
        let mut conv = MockConversationService::new();
        conv.expect_create_conversation()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_group(ws)) }));

        let svc = make_services(conv);
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(ws.to_string()),
        );
        args.insert("member_ids".to_string(), member_ids_arg(&members));
        let result = super::create_group(&svc, actor(), args, None)
            .await
            .expect("create_group with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace.
    #[tokio::test]
    async fn create_group_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let members = vec![UserId::new()];
        let mut conv = MockConversationService::new();
        conv.expect_create_conversation()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_group(ws)) }));

        let svc = make_services(conv);
        let mut args = serde_json::Map::new();
        args.insert("member_ids".to_string(), member_ids_arg(&members));
        let result = super::create_group(&svc, actor(), args, Some(ctx_with(ws)))
            .await
            .expect("create_group with context workspace must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn create_group_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        let members = vec![UserId::new()];
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut conv = MockConversationService::new();
        conv.expect_create_conversation()
            .withf(move |cmd| cmd.server_id == arg_ws_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_group(arg_ws)) }));

        let svc = make_services(conv);
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(arg_ws.to_string()),
        );
        args.insert("member_ids".to_string(), member_ids_arg(&members));
        let result = super::create_group(&svc, actor(), args, Some(ctx_with(ctx_ws)))
            .await
            .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation error mentions both paths.
    #[tokio::test]
    async fn create_group_both_absent_returns_validation() {
        let members = vec![UserId::new()];
        let svc = make_services(MockConversationService::new());
        let mut args = serde_json::Map::new();
        args.insert("member_ids".to_string(), member_ids_arg(&members));
        let err = super::create_group(&svc, actor(), args, None)
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

    // ── Track 1.1 membership/archive commands ──

    fn ctx_with_conversation(ws: ServerId, conversation_id: ConversationId) -> CommandContextDto {
        CommandContextDto {
            conversation_id: Some(conversation_id),
            ..ctx_with(ws)
        }
    }

    fn channel_dto(ws: ServerId, id: ConversationId) -> ConversationDto {
        ConversationDto {
            id,
            conversation_type: ConversationType::Channel,
            ..fake_group(ws)
        }
    }

    /// Palette/keybind path: the focused conversation comes from the context.
    #[tokio::test]
    async fn join_channel_uses_context_conversation() {
        let ws = ServerId::new();
        let cid = ConversationId::new();
        let mut conv = MockConversationService::new();
        conv.expect_join_channel()
            .withf(move |cmd| cmd.server_id == ws && cmd.conversation_id == cid)
            .return_once(move |_| Box::pin(async move { Ok(channel_dto(ws, cid)) }));
        let svc = make_services(conv);
        let result = super::join_channel(
            &svc,
            actor(),
            serde_json::Map::new(),
            Some(ctx_with_conversation(ws, cid)),
        )
        .await
        .expect("join");
        assert_eq!(result.status, CommandResultStatusDto::Success);
        assert_eq!(
            result.payload.expect("payload")["conversation_id"],
            cid.to_string()
        );
    }

    /// Agent path: an explicit `conversation_id` arg wins over the context.
    #[tokio::test]
    async fn leave_conversation_arg_wins_over_context() {
        let ws = ServerId::new();
        let arg_cid = ConversationId::new();
        let mut conv = MockConversationService::new();
        conv.expect_leave_conversation()
            .withf(move |cmd| cmd.conversation_id == arg_cid)
            .return_once(|_| Box::pin(async { Ok(()) }));
        let svc = make_services(conv);
        let mut args = serde_json::Map::new();
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(arg_cid.to_string()),
        );
        super::leave_conversation(
            &svc,
            actor(),
            args,
            Some(ctx_with_conversation(ws, ConversationId::new())),
        )
        .await
        .expect("leave");
    }

    #[tokio::test]
    async fn membership_command_without_conversation_is_validation() {
        let svc = make_services(MockConversationService::new());
        let err = super::join_channel(
            &svc,
            actor(),
            serde_json::Map::new(),
            Some(ctx_with(ServerId::new())),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, rim_services::ServiceError::Validation(msg) if msg.contains("conversation_id")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn invite_to_conversation_passes_user_ids() {
        let ws = ServerId::new();
        let cid = ConversationId::new();
        let users = vec![UserId::new(), UserId::new()];
        let expected = users.clone();
        let mut conv = MockConversationService::new();
        conv.expect_add_conversation_members()
            .withf(move |cmd| cmd.conversation_id == cid && cmd.user_ids == expected)
            .return_once(move |_| Box::pin(async move { Ok(channel_dto(ws, cid)) }));
        let svc = make_services(conv);
        let mut args = serde_json::Map::new();
        args.insert("user_ids".to_string(), member_ids_arg(&users));
        super::invite_to_conversation(&svc, actor(), args, Some(ctx_with_conversation(ws, cid)))
            .await
            .expect("invite");
    }

    #[tokio::test]
    async fn archive_and_unarchive_pass_the_flag() {
        for archived in [true, false] {
            let ws = ServerId::new();
            let cid = ConversationId::new();
            let mut conv = MockConversationService::new();
            conv.expect_set_conversation_archived()
                .withf(move |cmd| cmd.archived == archived && cmd.conversation_id == cid)
                .return_once(move |_| {
                    Box::pin(async move {
                        Ok(ConversationDto {
                            archived_at: archived.then(chrono::Utc::now),
                            ..channel_dto(ws, cid)
                        })
                    })
                });
            let svc = make_services(conv);
            let result = super::set_channel_archived(
                &svc,
                actor(),
                serde_json::Map::new(),
                Some(ctx_with_conversation(ws, cid)),
                archived,
            )
            .await
            .expect("archive flag");
            assert_eq!(result.payload.expect("payload")["archived"], archived);
        }
    }

    #[tokio::test]
    async fn remove_conversation_member_requires_user_id() {
        let ws = ServerId::new();
        let cid = ConversationId::new();
        let target = UserId::new();
        let svc = make_services(MockConversationService::new());
        let err = super::remove_conversation_member(
            &svc,
            actor(),
            serde_json::Map::new(),
            Some(ctx_with_conversation(ws, cid)),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));

        let mut conv = MockConversationService::new();
        conv.expect_remove_conversation_member()
            .withf(move |cmd| cmd.user_id == target && cmd.conversation_id == cid)
            .return_once(|_| Box::pin(async { Ok(()) }));
        let svc = make_services(conv);
        let mut args = serde_json::Map::new();
        args.insert(
            "user_id".to_string(),
            serde_json::Value::String(target.to_string()),
        );
        super::remove_conversation_member(
            &svc,
            actor(),
            args,
            Some(ctx_with_conversation(ws, cid)),
        )
        .await
        .expect("remove");
    }
}
