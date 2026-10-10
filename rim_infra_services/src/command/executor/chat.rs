use serde_json::Value;

use rim_services::conversation::command::{
    AddConversationMembersCmd, CreateConversationCmd, ListChannelsQuery, UpdateConversationTopicCmd,
};
use rim_services::conversation::types::ConversationParticipants;
use rim_services::message::command::{ListMessagesQuery, SearchMessagesQuery};
use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};

use super::{
    clamp_limit, optional_bool_arg, optional_integer_arg, optional_string_arg, parse_uuid_arg,
    parse_uuid_array_arg, require_string_arg, resolve_author_usernames, resolve_channel_titles,
    resolve_workspace_id,
};

/// Hard cap on the single-page result for `chat.list_channels`. Conservative
/// for Phase 3 (the AI surface uses this; an unbounded list would balloon
/// the agent's token budget on workspaces with thousands of channels).
/// When pagination lands (Phase 5), this becomes the per-page default and
/// a `cursor` arg gets exposed.
const CHAT_LIST_CHANNELS_LIMIT: u32 = 100;

pub(super) async fn list_channels(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    // P3-A2: first AI-callable backend builtin. The Phase 2 cap gate
    // already verified `chat.read` before we got here — this arm is
    // a thin adapter from typed args → ConversationService query →
    // JSON payload. `dir = After` + a generous `limit` mirrors the
    // existing palette/agent expectation that "list" returns a
    // single page of channels (pagination is a Phase 5 concern; the
    // tool surface today is "give me the channels").
    let server_id = resolve_workspace_id::<rim_core::server::model::ServerId>(
        &args,
        context.as_ref(),
        "server_id",
    )?;

    let page = services
        .conversation_service
        .list_channels(ListChannelsQuery {
            actor,
            server_id,
            filter: Default::default(),
            dir: rim_core::repository::pagination::PageDir::After,
            cursor: None,
            limit: CHAT_LIST_CHANNELS_LIMIT,
        })
        .await?;

    // Project to a stable wire shape for the AI tool consumer:
    // {id, title, conversation_type, is_private, is_archived, is_member}.
    // The frontend agent renders this directly; do NOT leak member_ids
    // (PII surface widening for a "list" tool is a Widow no-go) — the
    // caller's own membership is derived from them here instead.
    // Track 1.1: the flags let the agent and macros decide whether to
    // join (public, not a member) or invite, and skip archived channels.
    let channels: Vec<serde_json::Value> = page
        .items
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id.to_string(),
                "title": c.title,
                "conversation_type": c.conversation_type.as_str(),
                "is_private": c.is_private,
                "is_archived": c.is_archived(),
                "is_member": c.is_member(actor.user_id),
            })
        })
        .collect();

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: format!("{} channels found", channels.len()),
        payload: Some(serde_json::json!({ "channels": channels })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

pub(super) async fn create_channel(
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
    let name = require_string_arg(&args, "name")?;
    // P4-A4: optional `description` arg added for the AI surface.
    // The conversation create call has no description field, so we
    // create-then-update_topic. If `update_topic` fails after the
    // channel is already created we propagate the error — the
    // partial-success is visible to the caller (channel exists,
    // topic missing) which matches what they'd see in the UI if
    // they did this manually.
    let description = optional_string_arg(&args, "description");
    let is_private = optional_bool_arg(&args, "is_private").unwrap_or(false);
    // Track 1.1: optional initial members, added right after creation (the same two-step flow
    // the client uses for a private channel's first members).
    let member_ids = if args.contains_key("member_ids") {
        parse_uuid_array_arg::<rim_core::user::model::UserId>(&args, "member_ids")?
    } else {
        vec![]
    };

    let conversation = services
        .conversation_service
        .create_conversation(CreateConversationCmd {
            actor,
            server_id,
            participants: ConversationParticipants::Channel,
            title: Some(name),
            is_private,
            encrypted: false,
        })
        .await?;

    if !member_ids.is_empty() {
        services
            .conversation_service
            .add_conversation_members(AddConversationMembersCmd {
                actor,
                server_id,
                conversation_id: conversation.id,
                user_ids: member_ids,
            })
            .await?;
    }

    if let Some(desc) = description.filter(|s| !s.is_empty()) {
        services
            .conversation_service
            .update_topic(UpdateConversationTopicCmd {
                actor,
                server_id,
                conversation_id: conversation.id,
                topic: Some(desc),
            })
            .await?;
    }

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "channel created".to_string(),
        payload: Some(serde_json::json!({
            "conversation_id": conversation.id.to_string(),
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

pub(super) async fn mark_as_read(
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

    services
        .read_state_service
        .mark_as_read(rim_services::read_state::dto::MarkAsReadCmd {
            actor,
            workspace_id: server_id,
            conversation_id,
            read_at: chrono::Utc::now(),
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "conversation marked as read".to_string(),
        payload: None,
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

pub(super) async fn mark_all_as_read(
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

    services
        .read_state_service
        .mark_all_as_read(rim_services::read_state::dto::MarkAllAsReadCmd {
            actor,
            workspace_id: server_id,
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "all conversations marked as read".to_string(),
        payload: None,
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

pub(super) async fn search_messages(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    // P4-A5: server-side resolution of author usernames AND channel
    // titles via the batch `get_users_by_ids` / `get_conversations_by_ids`
    // APIs — one lookup each, so the author resolution is O(1) in
    // DB roundtrips regardless of how many results come back. This
    // kills the FE `memberResolver.ts` workaround: the agent consumes
    // {author: username} directly.
    let server_id = resolve_workspace_id::<rim_core::server::model::ServerId>(
        &args,
        context.as_ref(),
        "server_id",
    )?;
    let query_text = require_string_arg(&args, "query")?;
    let conversation_id = if args.contains_key("conversation_id") {
        Some(parse_uuid_arg::<
            rim_core::conversation::model::ConversationId,
        >(&args, "conversation_id")?)
    } else {
        None
    };
    let limit = clamp_limit(optional_integer_arg(&args, "limit"), 10, 50);

    let page = services
        .message_service
        .search_messages(SearchMessagesQuery {
            actor,
            server_id,
            conversation_id,
            query: query_text,
            limit,
            cursor: None,
            dir: rim_core::repository::pagination::PageDir::After,
        })
        .await?;

    // Project the message half of each hit for the batch author/channel resolution helpers,
    // which only need `&[MessageDto]`.
    let results: Vec<rim_services::message::dto::MessageDto> =
        page.items.iter().map(|hit| hit.message.clone()).collect();
    let usernames = resolve_author_usernames(services, &results).await?;
    let channel_titles = resolve_channel_titles(services, &results).await?;

    let projected: Vec<Value> = page
        .items
        .iter()
        .map(|hit| {
            let m = &hit.message;
            serde_json::json!({
                "id": m.id.to_string(),
                "channel": channel_titles
                    .get(&m.conversation_id)
                    .cloned()
                    .unwrap_or_default(),
                "author": usernames
                    .get(&m.author_id)
                    .cloned()
                    .unwrap_or_default(),
                "text": m.text.clone().unwrap_or_default(),
                "sentAt": m.sent_at.to_rfc3339(),
                "rank": hit.rank,
                "headline": hit.headline.clone(),
            })
        })
        .collect();

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: format!("{} results", projected.len()),
        payload: Some(serde_json::json!({ "results": projected })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

pub(super) async fn get_recent_messages(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    // P4-A5: read the newest page of messages in a conversation.
    // `PageDir::Before` + no cursor = "most recent" per the
    // pagination contract. Deleted messages are filtered server-side
    // so the agent never sees tombstoned content. Author usernames
    // resolved via the batch `get_users_by_ids` helper (no N+1).
    let server_id = resolve_workspace_id::<rim_core::server::model::ServerId>(
        &args,
        context.as_ref(),
        "server_id",
    )?;
    let conversation_id =
        parse_uuid_arg::<rim_core::conversation::model::ConversationId>(&args, "conversation_id")?;
    let limit = clamp_limit(optional_integer_arg(&args, "limit"), 30, 100);

    let page = services
        .message_service
        .list_messages(ListMessagesQuery {
            actor,
            server_id,
            conversation_id,
            dir: rim_core::repository::pagination::PageDir::Before,
            cursor: None,
            limit,
            include_deleted: false,
        })
        .await?;

    let usernames = resolve_author_usernames(services, &page.items).await?;

    let projected: Vec<Value> = page
        .items
        .iter()
        .map(|m| {
            serde_json::json!({
                "author": usernames
                    .get(&m.author_id)
                    .cloned()
                    .unwrap_or_default(),
                "text": m.text.clone().unwrap_or_default(),
                "sentAt": m.sent_at.to_rfc3339(),
            })
        })
        .collect();

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: format!("{} messages", projected.len()),
        payload: Some(serde_json::json!({ "messages": projected })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::model::{ConversationId, ConversationType};
    use rim_core::message::model::MessageId;
    use rim_core::repository::pagination::Page;
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::Actor;
    use rim_services::auth::service::MockAuthService;
    use rim_services::capability::service::MockCapabilityService;
    use rim_services::command::dto::{CommandContextDto, CommandResultStatusDto};
    use rim_services::conversation::dto::ConversationDto;
    use rim_services::conversation::service::MockConversationService;
    use rim_services::message::dto::MessageDto;
    use rim_services::message::service::MockMessageService;
    use rim_services::pin::service::MockPinService;
    use rim_services::read_state::service::MockReadStateService;
    use rim_services::server_member::service::MockServerMemberService;
    use rim_services::user::dto::UserDto;
    use rim_services::user::service::MockUserService;
    use rim_services::workspace_state::service::MockWorkspaceStateService;

    use crate::command::CommandServices;

    fn actor() -> Actor {
        Actor::new(UserId::new())
    }

    fn make_services(conv: MockConversationService, msg: MockMessageService) -> CommandServices {
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(conv),
            message_service: Arc::new(msg),
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

    fn one_channel_page(
        ws: ServerId,
    ) -> rim_core::repository::pagination::Page<
        ConversationDto,
        rim_services::conversation::dto::ChannelPageCursor,
    > {
        Page::new(
            vec![ConversationDto {
                id: ConversationId::new(),
                server_id: ws,
                conversation_type: ConversationType::Channel,
                title: Some("test-channel".to_string()),
                topic: None,
                member_ids: vec![],
                is_private: false,
                archived_at: None,
                created_by: None,
                slow_mode_seconds: 0,
                encrypted_at: None,
            }],
            None,
            None,
        )
    }

    // ── list_channels ────────────────────────────────────────────────────────

    /// Track 1.1: each channel carries `is_private`, `is_archived` and the caller's
    /// `is_member` (derived from member ids, which themselves never reach the payload).
    #[tokio::test]
    async fn list_channels_projects_privacy_archive_and_membership_flags() {
        let ws = ServerId::new();
        let caller = actor();
        let mut joined = one_channel_page(ws).items.remove(0);
        joined.member_ids = vec![caller.user_id];
        let mut private_archived = one_channel_page(ws).items.remove(0);
        private_archived.is_private = true;
        private_archived.archived_at = Some(chrono::Utc::now());
        let mut conv = MockConversationService::new();
        conv.expect_list_channels().return_once(move |_| {
            Box::pin(async move { Ok(Page::new(vec![joined, private_archived], None, None)) })
        });
        let svc = make_services(conv, MockMessageService::new());
        let result = super::list_channels(&svc, caller, serde_json::Map::new(), Some(ctx_with(ws)))
            .await
            .expect("list channels");
        let payload = result.payload.expect("payload");
        let channels = payload["channels"].as_array().expect("channels");
        assert_eq!(channels[0]["is_member"], true);
        assert_eq!(channels[0]["is_private"], false);
        assert_eq!(channels[0]["is_archived"], false);
        assert_eq!(channels[1]["is_member"], false);
        assert_eq!(channels[1]["is_private"], true);
        assert_eq!(channels[1]["is_archived"], true);
        assert!(channels[0].get("member_ids").is_none());
    }

    /// 1. AI path: args carry server_id, context is None — downstream sees arg UUID.
    #[tokio::test]
    async fn list_channels_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let ws_for_check = ws;
        let mut conv = MockConversationService::new();
        conv.expect_list_channels()
            .withf(move |q| q.server_id == ws_for_check)
            .return_once(move |_| Box::pin(async move { Ok(one_channel_page(ws)) }));

        let svc = make_services(conv, MockMessageService::new());
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(ws.to_string()),
        );
        let result = super::list_channels(&svc, actor(), args, None)
            .await
            .expect("list_channels with arg server_id must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: args have no server_id, context carries workspace_id — downstream sees context UUID.
    #[tokio::test]
    async fn list_channels_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let ws_for_check = ws;
        let mut conv = MockConversationService::new();
        conv.expect_list_channels()
            .withf(move |q| q.server_id == ws_for_check)
            .return_once(move |_| Box::pin(async move { Ok(one_channel_page(ws)) }));

        let svc = make_services(conv, MockMessageService::new());
        let result =
            super::list_channels(&svc, actor(), serde_json::Map::new(), Some(ctx_with(ws)))
                .await
                .expect("list_channels with context workspace_id must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied with different UUIDs — arg wins.
    #[tokio::test]
    async fn list_channels_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut conv = MockConversationService::new();
        conv.expect_list_channels()
            .withf(move |q| q.server_id == arg_ws_check)
            .return_once(move |_| Box::pin(async move { Ok(one_channel_page(arg_ws)) }));

        let svc = make_services(conv, MockMessageService::new());
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(arg_ws.to_string()),
        );
        let result = super::list_channels(&svc, actor(), args, Some(ctx_with(ctx_ws)))
            .await
            .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither arg nor context — Validation error mentions both paths.
    #[tokio::test]
    async fn list_channels_both_absent_returns_validation() {
        let svc = make_services(MockConversationService::new(), MockMessageService::new());
        let err = super::list_channels(&svc, actor(), serde_json::Map::new(), None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, rim_services::ServiceError::Validation(_)),
            "expected Validation, got: {err:?}"
        );
        if let rim_services::ServiceError::Validation(msg) = err {
            assert!(
                msg.contains("server_id"),
                "error must mention `server_id`; got: {msg}"
            );
            assert!(
                msg.contains("context.workspace_id"),
                "error must mention `context.workspace_id`; got: {msg}"
            );
        }
    }

    // ── create_channel: Track 1.1 private + initial members ─────────────────

    fn fake_channel(ws: ServerId) -> ConversationDto {
        ConversationDto {
            id: ConversationId::new(),
            server_id: ws,
            conversation_type: ConversationType::Channel,
            title: Some("new-ch".to_string()),
            topic: None,
            member_ids: vec![],
            is_private: false,
            archived_at: None,
            created_by: None,
            slow_mode_seconds: 0,
            encrypted_at: None,
        }
    }

    #[tokio::test]
    async fn create_channel_private_with_members_adds_them_after_creation() {
        let ws = ServerId::new();
        let members = vec![UserId::new(), UserId::new()];
        let expected = members.clone();
        let mut conv = MockConversationService::new();
        let created = ConversationDto {
            is_private: true,
            ..fake_channel(ws)
        };
        let created_id = created.id;
        conv.expect_create_conversation()
            .withf(|cmd| cmd.is_private)
            .return_once(move |_| Box::pin(async move { Ok(created) }));
        conv.expect_add_conversation_members()
            .withf(move |cmd| cmd.conversation_id == created_id && cmd.user_ids == expected)
            .return_once(move |_| Box::pin(async move { Ok(fake_channel(ws)) }));

        let svc = make_services(conv, MockMessageService::new());
        let mut args = serde_json::Map::new();
        args.insert("name".to_string(), serde_json::json!("hiring"));
        args.insert("is_private".to_string(), serde_json::json!("true"));
        args.insert(
            "member_ids".to_string(),
            serde_json::Value::Array(
                members
                    .iter()
                    .map(|id| serde_json::Value::String(id.to_string()))
                    .collect(),
            ),
        );
        super::create_channel(&svc, actor(), args, Some(ctx_with(ws)))
            .await
            .expect("private channel with members");
    }

    #[tokio::test]
    async fn create_channel_defaults_to_public_without_members() {
        let ws = ServerId::new();
        let mut conv = MockConversationService::new();
        conv.expect_create_conversation()
            .withf(|cmd| !cmd.is_private)
            .return_once(move |_| Box::pin(async move { Ok(fake_channel(ws)) }));
        conv.expect_add_conversation_members().never();
        let svc = make_services(conv, MockMessageService::new());
        let mut args = serde_json::Map::new();
        args.insert("name".to_string(), serde_json::json!("general-2"));
        super::create_channel(&svc, actor(), args, Some(ctx_with(ws)))
            .await
            .expect("public channel");
    }

    // ── create_channel ───────────────────────────────────────────────────────

    /// 1. AI path: args carry server_id — downstream sees arg UUID.
    #[tokio::test]
    async fn create_channel_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut conv = MockConversationService::new();
        conv.expect_create_conversation()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(ConversationDto {
                        id: ConversationId::new(),
                        server_id: ws,
                        conversation_type: ConversationType::Channel,
                        title: Some("new-ch".to_string()),
                        topic: None,
                        member_ids: vec![],
                        is_private: false,
                        archived_at: None,
                        created_by: None,
                        slow_mode_seconds: 0,
                        encrypted_at: None,
                    })
                })
            });

        let svc = make_services(conv, MockMessageService::new());
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(ws.to_string()),
        );
        args.insert(
            "name".to_string(),
            serde_json::Value::String("new-ch".to_string()),
        );
        let result = super::create_channel(&svc, actor(), args, None)
            .await
            .expect("create_channel with arg server_id must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: args have no server_id, context carries workspace_id.
    #[tokio::test]
    async fn create_channel_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut conv = MockConversationService::new();
        conv.expect_create_conversation()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(ConversationDto {
                        id: ConversationId::new(),
                        server_id: ws,
                        conversation_type: ConversationType::Channel,
                        title: Some("new-ch".to_string()),
                        topic: None,
                        member_ids: vec![],
                        is_private: false,
                        archived_at: None,
                        created_by: None,
                        slow_mode_seconds: 0,
                        encrypted_at: None,
                    })
                })
            });

        let svc = make_services(conv, MockMessageService::new());
        let mut args = serde_json::Map::new();
        args.insert(
            "name".to_string(),
            serde_json::Value::String("new-ch".to_string()),
        );
        let result = super::create_channel(&svc, actor(), args, Some(ctx_with(ws)))
            .await
            .expect("create_channel with context workspace_id must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn create_channel_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut conv = MockConversationService::new();
        conv.expect_create_conversation()
            .withf(move |cmd| cmd.server_id == arg_ws_check)
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(ConversationDto {
                        id: ConversationId::new(),
                        server_id: arg_ws,
                        conversation_type: ConversationType::Channel,
                        title: None,
                        topic: None,
                        member_ids: vec![],
                        is_private: false,
                        archived_at: None,
                        created_by: None,
                        slow_mode_seconds: 0,
                        encrypted_at: None,
                    })
                })
            });

        let svc = make_services(conv, MockMessageService::new());
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(arg_ws.to_string()),
        );
        args.insert(
            "name".to_string(),
            serde_json::Value::String("ch".to_string()),
        );
        let result = super::create_channel(&svc, actor(), args, Some(ctx_with(ctx_ws)))
            .await
            .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation error mentions both paths.
    #[tokio::test]
    async fn create_channel_both_absent_returns_validation() {
        let svc = make_services(MockConversationService::new(), MockMessageService::new());
        let mut args = serde_json::Map::new();
        args.insert(
            "name".to_string(),
            serde_json::Value::String("ch".to_string()),
        );
        let err = super::create_channel(&svc, actor(), args, None)
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

    // ── mark_as_read ─────────────────────────────────────────────────────────

    /// 1. AI path: args carry server_id.
    #[tokio::test]
    async fn mark_as_read_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let conv_id = ConversationId::new();
        let ws_check = ws;
        let conv_check = conv_id;
        let mut read_svc = MockReadStateService::new();
        read_svc
            .expect_mark_as_read()
            .withf(move |cmd| cmd.workspace_id == ws_check && cmd.conversation_id == conv_check)
            .return_once(|_| Box::pin(async { Ok(()) }));

        let services = CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(MockPinService::new()),
            read_state_service: Arc::new(read_svc),
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
        };

        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(ws.to_string()),
        );
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(conv_id.to_string()),
        );
        let result = super::mark_as_read(&services, actor(), args, None)
            .await
            .expect("mark_as_read with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace_id.
    #[tokio::test]
    async fn mark_as_read_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let conv_id = ConversationId::new();
        let ws_check = ws;
        let conv_check = conv_id;
        let mut read_svc = MockReadStateService::new();
        read_svc
            .expect_mark_as_read()
            .withf(move |cmd| cmd.workspace_id == ws_check && cmd.conversation_id == conv_check)
            .return_once(|_| Box::pin(async { Ok(()) }));

        let services = CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(MockPinService::new()),
            read_state_service: Arc::new(read_svc),
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
        };

        let mut args = serde_json::Map::new();
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(conv_id.to_string()),
        );
        let result = super::mark_as_read(&services, actor(), args, Some(ctx_with(ws)))
            .await
            .expect("mark_as_read with context workspace_id must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn mark_as_read_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        let conv_id = ConversationId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut read_svc = MockReadStateService::new();
        read_svc
            .expect_mark_as_read()
            .withf(move |cmd| cmd.workspace_id == arg_ws_check)
            .return_once(|_| Box::pin(async { Ok(()) }));

        let services = CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(MockPinService::new()),
            read_state_service: Arc::new(read_svc),
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
        };

        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(arg_ws.to_string()),
        );
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(conv_id.to_string()),
        );
        let result = super::mark_as_read(&services, actor(), args, Some(ctx_with(ctx_ws)))
            .await
            .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation.
    #[tokio::test]
    async fn mark_as_read_both_absent_returns_validation() {
        let conv_id = ConversationId::new();
        let svc = make_services(MockConversationService::new(), MockMessageService::new());
        let mut args = serde_json::Map::new();
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(conv_id.to_string()),
        );
        let err = super::mark_as_read(&svc, actor(), args, None)
            .await
            .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
        if let rim_services::ServiceError::Validation(msg) = err {
            assert!(msg.contains("server_id"));
            assert!(msg.contains("context.workspace_id"));
        }
    }

    // ── mark_all_as_read ─────────────────────────────────────────────────────

    /// 1. AI path: args carry server_id.
    #[tokio::test]
    async fn mark_all_as_read_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut read_svc = MockReadStateService::new();
        read_svc
            .expect_mark_all_as_read()
            .withf(move |cmd| cmd.workspace_id == ws_check)
            .return_once(|_| Box::pin(async { Ok(()) }));

        let services = CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(MockPinService::new()),
            read_state_service: Arc::new(read_svc),
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
        };

        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(ws.to_string()),
        );
        let result = super::mark_all_as_read(&services, actor(), args, None)
            .await
            .expect("mark_all_as_read with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace_id.
    #[tokio::test]
    async fn mark_all_as_read_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut read_svc = MockReadStateService::new();
        read_svc
            .expect_mark_all_as_read()
            .withf(move |cmd| cmd.workspace_id == ws_check)
            .return_once(|_| Box::pin(async { Ok(()) }));

        let services = CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(MockPinService::new()),
            read_state_service: Arc::new(read_svc),
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
        };

        let result = super::mark_all_as_read(
            &services,
            actor(),
            serde_json::Map::new(),
            Some(ctx_with(ws)),
        )
        .await
        .expect("mark_all_as_read with context workspace_id must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn mark_all_as_read_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut read_svc = MockReadStateService::new();
        read_svc
            .expect_mark_all_as_read()
            .withf(move |cmd| cmd.workspace_id == arg_ws_check)
            .return_once(|_| Box::pin(async { Ok(()) }));

        let services = CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(MockPinService::new()),
            read_state_service: Arc::new(read_svc),
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
        };

        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(arg_ws.to_string()),
        );
        let result = super::mark_all_as_read(&services, actor(), args, Some(ctx_with(ctx_ws)))
            .await
            .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation.
    #[tokio::test]
    async fn mark_all_as_read_both_absent_returns_validation() {
        let svc = make_services(MockConversationService::new(), MockMessageService::new());
        let err = super::mark_all_as_read(&svc, actor(), serde_json::Map::new(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
        if let rim_services::ServiceError::Validation(msg) = err {
            assert!(msg.contains("server_id"));
            assert!(msg.contains("context.workspace_id"));
        }
    }

    // ── search_messages ──────────────────────────────────────────────────────

    fn make_services_with_msg(msg_svc: MockMessageService) -> CommandServices {
        let mut user_svc = MockUserService::new();
        user_svc
            .expect_get_users_by_ids()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let mut conv_svc = MockConversationService::new();
        conv_svc
            .expect_get_conversations_by_ids()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(conv_svc),
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
            moderation_service: std::sync::Arc::new(
                rim_services::moderation::service::MockModerationService::no_timeouts(),
            ),
            notification_service: std::sync::Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
        }
    }

    /// 1. AI path: args carry server_id.
    #[tokio::test]
    async fn search_messages_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut msg_svc = MockMessageService::new();
        msg_svc
            .expect_search_messages()
            .withf(move |q| q.server_id == ws_check)
            .return_once(|_| {
                Box::pin(async {
                    Ok(rim_core::repository::pagination::Page::new(
                        vec![],
                        None,
                        None,
                    ))
                })
            });

        let svc = make_services_with_msg(msg_svc);
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(ws.to_string()),
        );
        args.insert(
            "query".to_string(),
            serde_json::Value::String("hello".to_string()),
        );
        let result = super::search_messages(&svc, actor(), args, None)
            .await
            .expect("search_messages with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id, context carries workspace.
    #[tokio::test]
    async fn search_messages_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut msg_svc = MockMessageService::new();
        msg_svc
            .expect_search_messages()
            .withf(move |q| q.server_id == ws_check)
            .return_once(|_| {
                Box::pin(async {
                    Ok(rim_core::repository::pagination::Page::new(
                        vec![],
                        None,
                        None,
                    ))
                })
            });

        let svc = make_services_with_msg(msg_svc);
        let mut args = serde_json::Map::new();
        args.insert(
            "query".to_string(),
            serde_json::Value::String("hello".to_string()),
        );
        let result = super::search_messages(&svc, actor(), args, Some(ctx_with(ws)))
            .await
            .expect("search_messages with context must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn search_messages_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut msg_svc = MockMessageService::new();
        msg_svc
            .expect_search_messages()
            .withf(move |q| q.server_id == arg_ws_check)
            .return_once(|_| {
                Box::pin(async {
                    Ok(rim_core::repository::pagination::Page::new(
                        vec![],
                        None,
                        None,
                    ))
                })
            });

        let svc = make_services_with_msg(msg_svc);
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(arg_ws.to_string()),
        );
        args.insert(
            "query".to_string(),
            serde_json::Value::String("hello".to_string()),
        );
        let result = super::search_messages(&svc, actor(), args, Some(ctx_with(ctx_ws)))
            .await
            .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation.
    #[tokio::test]
    async fn search_messages_both_absent_returns_validation() {
        let svc = make_services_with_msg(MockMessageService::new());
        let mut args = serde_json::Map::new();
        args.insert(
            "query".to_string(),
            serde_json::Value::String("hello".to_string()),
        );
        let err = super::search_messages(&svc, actor(), args, None)
            .await
            .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
        if let rim_services::ServiceError::Validation(msg) = err {
            assert!(msg.contains("server_id"));
            assert!(msg.contains("context.workspace_id"));
        }
    }

    // ── get_recent_messages ──────────────────────────────────────────────────

    fn make_services_with_msg_and_user(
        msg_svc: MockMessageService,
        user_svc: MockUserService,
    ) -> CommandServices {
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
            user_service: Arc::new(user_svc),
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

    fn empty_msg_page() -> rim_core::repository::pagination::Page<
        MessageDto,
        rim_services::message::dto::MessagePageCursor,
    > {
        Page::new(vec![], None, None)
    }

    /// 1. AI path: args carry server_id.
    #[tokio::test]
    async fn get_recent_messages_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let conv_id = ConversationId::new();
        let ws_check = ws;
        let conv_check = conv_id;
        let mut msg_svc = MockMessageService::new();
        msg_svc
            .expect_list_messages()
            .withf(move |q| q.server_id == ws_check && q.conversation_id == conv_check)
            .return_once(|_| Box::pin(async { Ok(empty_msg_page()) }));

        let mut user_svc = MockUserService::new();
        user_svc
            .expect_get_users_by_ids()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let svc = make_services_with_msg_and_user(msg_svc, user_svc);
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(ws.to_string()),
        );
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(conv_id.to_string()),
        );
        let result = super::get_recent_messages(&svc, actor(), args, None)
            .await
            .expect("get_recent_messages with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace.
    #[tokio::test]
    async fn get_recent_messages_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let conv_id = ConversationId::new();
        let ws_check = ws;
        let conv_check = conv_id;
        let mut msg_svc = MockMessageService::new();
        msg_svc
            .expect_list_messages()
            .withf(move |q| q.server_id == ws_check && q.conversation_id == conv_check)
            .return_once(|_| Box::pin(async { Ok(empty_msg_page()) }));

        let mut user_svc = MockUserService::new();
        user_svc
            .expect_get_users_by_ids()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let svc = make_services_with_msg_and_user(msg_svc, user_svc);
        let mut args = serde_json::Map::new();
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(conv_id.to_string()),
        );
        let result = super::get_recent_messages(&svc, actor(), args, Some(ctx_with(ws)))
            .await
            .expect("get_recent_messages with context must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn get_recent_messages_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        let conv_id = ConversationId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut msg_svc = MockMessageService::new();
        msg_svc
            .expect_list_messages()
            .withf(move |q| q.server_id == arg_ws_check)
            .return_once(|_| Box::pin(async { Ok(empty_msg_page()) }));

        let mut user_svc = MockUserService::new();
        user_svc
            .expect_get_users_by_ids()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let svc = make_services_with_msg_and_user(msg_svc, user_svc);
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(arg_ws.to_string()),
        );
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(conv_id.to_string()),
        );
        let result = super::get_recent_messages(&svc, actor(), args, Some(ctx_with(ctx_ws)))
            .await
            .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation.
    #[tokio::test]
    async fn get_recent_messages_both_absent_returns_validation() {
        let conv_id = ConversationId::new();
        let svc = make_services(MockConversationService::new(), MockMessageService::new());
        let mut args = serde_json::Map::new();
        args.insert(
            "conversation_id".to_string(),
            serde_json::Value::String(conv_id.to_string()),
        );
        let err = super::get_recent_messages(&svc, actor(), args, None)
            .await
            .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
        if let rim_services::ServiceError::Validation(msg) = err {
            assert!(msg.contains("server_id"));
            assert!(msg.contains("context.workspace_id"));
        }
    }

    // ── Unused imports silenced ───────────────────────────────────────────────
    // (MessageId, UserDto used in helper fn signatures above)
    fn _use_imports(_: MessageId, _: UserDto) {}
}
