use serde_json::Value;

use rim_services::conversation::command::{
    CreateConversationCmd, ListChannelsQuery, UpdateConversationTopicCmd,
};
use rim_services::conversation::types::ConversationParticipants;
use rim_services::message::command::{ListMessagesQuery, SearchMessagesQuery};
use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandResultDto, CommandResultStatusDto};

use super::{
    clamp_limit, optional_integer_arg, optional_string_arg, parse_uuid_arg, require_string_arg,
    resolve_author_usernames, resolve_channel_titles,
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
) -> ServiceResult<CommandResultDto> {
    // P3-A2: first AI-callable backend builtin. The Phase 2 cap gate
    // already verified `chat.read` before we got here — this arm is
    // a thin adapter from typed args → ConversationService query →
    // JSON payload. `dir = After` + a generous `limit` mirrors the
    // existing palette/agent expectation that "list" returns a
    // single page of channels (pagination is a Phase 5 concern; the
    // tool surface today is "give me the channels").
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;

    let page = services
        .conversation_service
        .list_channels(ListChannelsQuery {
            actor,
            server_id,
            dir: rim_core::repository::pagination::PageDir::After,
            cursor: None,
            limit: CHAT_LIST_CHANNELS_LIMIT,
        })
        .await?;

    // Project to a stable wire shape for the AI tool consumer:
    // {id, title, conversation_type}. The frontend agent renders
    // this directly; do NOT leak member_ids (PII surface widening
    // for a "list" tool is a Widow no-go).
    let channels: Vec<serde_json::Value> = page
        .items
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id.to_string(),
                "title": c.title,
                "conversation_type": c.conversation_type.as_str(),
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
) -> ServiceResult<CommandResultDto> {
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
    let name = require_string_arg(&args, "name")?;
    // P4-A4: optional `description` arg added for the AI surface.
    // The conversation create call has no description field, so we
    // create-then-update_topic. If `update_topic` fails after the
    // channel is already created we propagate the error — the
    // partial-success is visible to the caller (channel exists,
    // topic missing) which matches what they'd see in the UI if
    // they did this manually.
    let description = optional_string_arg(&args, "description");

    let conversation = services
        .conversation_service
        .create_conversation(CreateConversationCmd {
            actor,
            server_id,
            participants: ConversationParticipants::Channel,
            title: Some(name),
        })
        .await?;

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
) -> ServiceResult<CommandResultDto> {
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
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
) -> ServiceResult<CommandResultDto> {
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;

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
) -> ServiceResult<CommandResultDto> {
    // P4-A5: server-side resolution of author usernames AND channel
    // titles via the batch `get_users_by_ids` / `get_conversations_by_ids`
    // APIs — one lookup each, so the author resolution is O(1) in
    // DB roundtrips regardless of how many results come back. This
    // kills the FE `memberResolver.ts` workaround: the agent consumes
    // {author: username} directly.
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
    let query_text = require_string_arg(&args, "query")?;
    let conversation_id = if args.contains_key("conversation_id") {
        Some(parse_uuid_arg::<
            rim_core::conversation::model::ConversationId,
        >(&args, "conversation_id")?)
    } else {
        None
    };
    let limit = clamp_limit(optional_integer_arg(&args, "limit"), 10, 50);

    let results = services
        .message_service
        .search_messages(SearchMessagesQuery {
            actor,
            server_id,
            conversation_id,
            query: query_text,
            limit,
            include_deleted: false,
        })
        .await?;

    let usernames = resolve_author_usernames(services, &results).await?;
    let channel_titles = resolve_channel_titles(services, &results).await?;

    let projected: Vec<Value> = results
        .iter()
        .map(|m| {
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
) -> ServiceResult<CommandResultDto> {
    // P4-A5: read the newest page of messages in a conversation.
    // `PageDir::Before` + no cursor = "most recent" per the
    // pagination contract. Deleted messages are filtered server-side
    // so the agent never sees tombstoned content. Author usernames
    // resolved via the batch `get_users_by_ids` helper (no N+1).
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
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
