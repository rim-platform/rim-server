mod admin;
mod chat;
mod chat_messages;
mod conversation;
mod desktop_local;
mod member;
mod pin;
mod reaction;
mod state;

use std::collections::HashMap;

use serde_json::Value;

use rim_services::message::dto::MessageDto;
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto};

pub(super) async fn execute_builtin_command(
    services: &CommandServices,
    actor: Actor,
    command_id: &str,
    args: Option<Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let args = require_object_args(args)?;

    match command_id {
        "chat.post_message" => chat_messages::post_message(services, actor, args).await,
        "chat.reply_to_selected_message" => {
            chat_messages::reply_to_selected_message(services, actor, args, context).await
        }
        "chat.delete_message" => {
            chat_messages::delete_message(services, actor, args, context).await
        }
        "chat.edit_message" => chat_messages::edit_message(services, actor, args, context).await,
        "chat.create_group" => conversation::create_group(services, actor, args).await,
        "chat.list_channels" => chat::list_channels(services, actor, args).await,
        "chat.create_channel" => chat::create_channel(services, actor, args).await,
        "chat.mark_as_read" => chat::mark_as_read(services, actor, args).await,
        "chat.mark_all_as_read" => chat::mark_all_as_read(services, actor, args).await,
        "chat.search_messages" => chat::search_messages(services, actor, args).await,
        "chat.get_recent_messages" => chat::get_recent_messages(services, actor, args).await,
        "chat.list_members" => member::list_members(services, actor, args).await,
        "chat.pin_message" => pin::pin_message(services, actor, args).await,
        "chat.toggle_reaction" => reaction::toggle_reaction(services, actor, args).await,
        "admin.invite_member" => admin::invite_member(services, actor, args).await,
        "state.get" => state::get(services, actor, args).await,
        "state.set" => state::set(services, actor, args).await,
        "state.delete" => state::delete(services, actor, args).await,
        "state.list" => state::list(services, actor, args).await,
        "state.incr" => state::incr(services, actor, args).await,
        id if desktop_local::is_desktop_local(id) => desktop_local::desktop_local_rejected(id),
        _ => Err(ServiceError::NotFound),
    }
}

// ── Shared arg-parsing helpers ──────────────────────────────────────────────

pub(super) fn require_object_args(
    args: Option<Value>,
) -> ServiceResult<serde_json::Map<String, Value>> {
    match args {
        Some(Value::Object(map)) => Ok(map),
        Some(_) => Err(ServiceError::Validation(
            "command args must be a JSON object".to_string(),
        )),
        None => Ok(serde_json::Map::new()),
    }
}

pub(super) fn require_string_arg(
    map: &serde_json::Map<String, Value>,
    key: &str,
) -> ServiceResult<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ServiceError::Validation(format!("missing string arg `{key}`")))
}

pub(super) fn optional_string_arg(
    map: &serde_json::Map<String, Value>,
    key: &str,
) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(|value| value.to_string())
}

/// Parse an optional integer arg that may arrive as a JSON number (preferred)
/// or a string (some agent providers coerce everything to strings before
/// calling tools). Missing/invalid values yield `None` so the caller can
/// apply a default; out-of-range values are rejected so callers can clamp.
pub(super) fn optional_integer_arg(map: &serde_json::Map<String, Value>, key: &str) -> Option<i64> {
    match map.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

/// Clamp an optional limit arg to `[1, max]` with a default when absent.
/// Values <= 0 fall back to `default`; values > `max` clip to `max`.
pub(super) fn clamp_limit(raw: Option<i64>, default: u32, max: u32) -> u32 {
    match raw {
        Some(v) if v >= 1 => std::cmp::min(v as u32, max),
        _ => default,
    }
}

pub(super) fn parse_uuid_arg<T>(map: &serde_json::Map<String, Value>, key: &str) -> ServiceResult<T>
where
    T: From<uuid::Uuid>,
{
    let raw = require_string_arg(map, key)?;
    let uuid = uuid::Uuid::parse_str(&raw)
        .map_err(|_| ServiceError::Validation(format!("invalid uuid arg `{key}`")))?;
    Ok(T::from(uuid))
}

pub(super) fn parse_uuid_array_arg<T>(
    map: &serde_json::Map<String, Value>,
    key: &str,
) -> ServiceResult<Vec<T>>
where
    T: From<uuid::Uuid>,
{
    let values = map
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| ServiceError::Validation(format!("missing array arg `{key}`")))?;

    values
        .iter()
        .map(|value| {
            let raw = value.as_str().ok_or_else(|| {
                ServiceError::Validation(format!("invalid array arg `{key}` element"))
            })?;
            let uuid = uuid::Uuid::parse_str(raw)
                .map_err(|_| ServiceError::Validation(format!("invalid uuid arg `{key}`")))?;
            Ok(T::from(uuid))
        })
        .collect()
}

pub(super) fn parse_scope_arg(
    args: &serde_json::Map<String, Value>,
) -> ServiceResult<rim_services::workspace_state::dto::WorkspaceStateScopeDto> {
    let scope_str = optional_string_arg(args, "scope").unwrap_or_else(|| "server".to_string());
    match scope_str.as_str() {
        "server" => Ok(rim_services::workspace_state::dto::WorkspaceStateScopeDto::Server),
        "user" => Ok(rim_services::workspace_state::dto::WorkspaceStateScopeDto::User),
        _ => Err(ServiceError::Validation(format!(
            "invalid scope `{scope_str}`, must be 'server' or 'user'"
        ))),
    }
}

// ── Shared resolution helpers ────────────────────────────────────────────────

/// Batch-resolve author user IDs → usernames for a list of messages. Uses
/// `UserService::get_users_by_ids` so the underlying repository can hit the
/// DB once (DataLoader-style) regardless of how many distinct authors the
/// result set contains. Returned map has exactly one entry per distinct
/// `author_id` that exists; unknown IDs are silently dropped (the caller
/// falls back to an empty string in that case — keeps tool output robust
/// when an author has been hard-deleted).
pub(super) async fn resolve_author_usernames(
    services: &CommandServices,
    messages: &[MessageDto],
) -> ServiceResult<HashMap<rim_core::user::model::UserId, String>> {
    if messages.is_empty() {
        return Ok(HashMap::new());
    }
    let unique: Vec<rim_core::user::model::UserId> = messages
        .iter()
        .map(|m| m.author_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let users = services.user_service.get_users_by_ids(&unique).await?;
    Ok(users.into_iter().map(|u| (u.id, u.username)).collect())
}

/// Batch-resolve conversation IDs → channel titles for a list of messages.
/// Same DataLoader rationale as `resolve_author_usernames`.
pub(super) async fn resolve_channel_titles(
    services: &CommandServices,
    messages: &[MessageDto],
) -> ServiceResult<HashMap<rim_core::conversation::model::ConversationId, String>> {
    if messages.is_empty() {
        return Ok(HashMap::new());
    }
    let unique: Vec<rim_core::conversation::model::ConversationId> = messages
        .iter()
        .map(|m| m.conversation_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let conversations = services
        .conversation_service
        .get_conversations_by_ids(&unique)
        .await?;
    Ok(conversations
        .into_iter()
        .map(|c| (c.id, c.title.unwrap_or_default()))
        .collect())
}

/// Given the current reaction groups on a message, return whether the
/// actor already has a reaction with the target emoji. `chat.toggle_reaction`
/// uses this to decide whether a toggle call added or removed (the backend
/// mutation is bidirectional; it doesn't hand us back a verb).
pub(super) fn reaction_has_actor(
    groups: &[rim_services::message_reaction::dto::ReactionGroupDto],
    emoji: &str,
    actor_user_id: rim_core::user::model::UserId,
) -> bool {
    groups
        .iter()
        .any(|g| g.emoji == emoji && g.user_ids.iter().any(|uid| *uid == actor_user_id))
}
