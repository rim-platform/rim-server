mod admin;
mod chat;
mod chat_messages;
mod conversation;
mod desktop_local;
mod incident;
mod member;
mod oncall;
mod pin;
mod reaction;
mod state;

use std::collections::HashMap;

use serde_json::Value;

use rim_services::command::initiator::CommandInitiator;
use rim_services::message::dto::MessageDto;
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto};

/// Dispatches a resolved builtin command id to its domain executor.
///
/// `initiator` (C2 §3b) is only consumed by the `incident.ack` /
/// `incident.escalate` arms, which thread it into
/// [`rim_services::incident::command::AcknowledgeIncidentCmd`] /
/// [`rim_services::incident::command::EscalateIncidentNowCmd`] so the
/// service layer can append an agent-transcript timeline note when the
/// initiator is agentic. Every other domain executor ignores it — this
/// function does not change any other arm's signature.
pub(super) async fn execute_builtin_command(
    services: &CommandServices,
    actor: Actor,
    command_id: &str,
    args: Option<Value>,
    context: Option<CommandContextDto>,
    initiator: CommandInitiator,
) -> ServiceResult<CommandResultDto> {
    let args = require_object_args(args)?;

    match command_id {
        "chat.post_message" => chat_messages::post_message(services, actor, args, context).await,
        "chat.reply_to_selected_message" => {
            chat_messages::reply_to_selected_message(services, actor, args, context).await
        }
        "chat.delete_message" => {
            chat_messages::delete_message(services, actor, args, context).await
        }
        "chat.edit_message" => chat_messages::edit_message(services, actor, args, context).await,
        "chat.create_group" => conversation::create_group(services, actor, args, context).await,
        "chat.list_channels" => chat::list_channels(services, actor, args, context).await,
        "chat.create_channel" => chat::create_channel(services, actor, args, context).await,
        "chat.mark_as_read" => chat::mark_as_read(services, actor, args, context).await,
        "chat.mark_all_as_read" => chat::mark_all_as_read(services, actor, args, context).await,
        "chat.search_messages" => chat::search_messages(services, actor, args, context).await,
        "chat.get_recent_messages" => {
            chat::get_recent_messages(services, actor, args, context).await
        }
        "chat.list_members" => member::list_members(services, actor, args, context).await,
        "chat.pin_message" => pin::pin_message(services, actor, args, context).await,
        "chat.toggle_reaction" => reaction::toggle_reaction(services, actor, args, context).await,
        "admin.invite_member" => admin::invite_member(services, actor, args, context).await,
        "incident.ack" => incident::ack(services, actor, args, context, initiator).await,
        "incident.escalate" => incident::escalate(services, actor, args, context, initiator).await,
        "oncall.who" => oncall::who(services, actor, args, context).await,
        "oncall.take_shift" => oncall::take_shift(services, actor, args, context).await,
        "state.get" => state::get(services, actor, args, context).await,
        "state.set" => state::set(services, actor, args, context).await,
        "state.delete" => state::delete(services, actor, args, context).await,
        "state.list" => state::list(services, actor, args, context).await,
        "state.incr" => state::incr(services, actor, args, context).await,
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

/// Parse a required RFC 3339 timestamp string arg (e.g. `"2026-07-10T15:00:00Z"`) into a UTC
/// `DateTime`. Used by `oncall.take_shift`, which takes an explicit override window rather than
/// an invented default duration.
pub(super) fn parse_rfc3339_arg(
    map: &serde_json::Map<String, Value>,
    key: &str,
) -> ServiceResult<chrono::DateTime<chrono::Utc>> {
    let raw = require_string_arg(map, key)?;
    chrono::DateTime::parse_from_rfc3339(&raw)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|_| ServiceError::Validation(format!("invalid RFC 3339 timestamp arg `{key}`")))
}

/// Resolve a `WorkspaceId`-typed argument with context fallback.
///
/// Precedence:
///   1. `args.<arg_name>` — used when the key is present and the value is a
///      non-empty string that parses as a valid UUID. The empty string and
///      whitespace-only strings are treated as **absent** (consistent with
///      `require_string_arg`).
///   2. `context.workspace_id` — used when the arg is absent OR empty/blank.
///      The `Option<&CommandContextDto>` is dereferenced with the
///      `.and_then(|c| c.workspace_id)` shape (i.e. either a missing context
///      or a context with `workspace_id: None` falls through to the error).
///   3. If neither path yields a value, returns
///      `ServiceError::Validation("missing workspace id: provide \
///       `<arg_name>` arg or `context.workspace_id`")`. The message MUST
///      mention BOTH paths so the developer debugging it knows what to
///      check.
///
/// A present-but-malformed arg is a hard error (returns
/// `Validation("invalid uuid arg `<arg_name>`")`). The function does NOT
/// silently fall through to context when the arg parses badly — that would
/// hide LLM tool-call bugs.
///
/// Generic over the target ID type so the same helper works for any
/// `WorkspaceId`-typed arg name. Today every call passes
/// `ServerId` and `"server_id"`, but the signature accepts any
/// `T: From<uuid::Uuid>` and any string key so future commands using a
/// different arg name (e.g. `workspace_id`) get free coverage.
///
/// # Examples
///
/// ```ignore
/// // Palette path — args is empty, context carries workspace.
/// let server_id = resolve_workspace_id::<ServerId>(&args, context.as_ref(), "server_id")?;
///
/// // AI path — args carries server_id, context is None.
/// // Same call site; arg wins.
/// let server_id = resolve_workspace_id::<ServerId>(&args, context.as_ref(), "server_id")?;
/// ```
pub(super) fn resolve_workspace_id<T>(
    args: &serde_json::Map<String, Value>,
    context: Option<&CommandContextDto>,
    arg_name: &str,
) -> ServiceResult<T>
where
    T: From<uuid::Uuid>,
{
    // 1. Try the arg.
    if let Some(raw) = args.get(arg_name).and_then(|v| v.as_str()) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            // Present and non-blank: parse strictly. A bad UUID here is a
            // hard error; we don't fall through to context because that
            // would mask LLM tool-call bugs.
            let uuid = uuid::Uuid::parse_str(trimmed)
                .map_err(|_| ServiceError::Validation(format!("invalid uuid arg `{arg_name}`")))?;
            return Ok(T::from(uuid));
        }
    }

    // 2. Fall back to context.
    if let Some(ws) = context.and_then(|c| c.workspace_id) {
        // ServerId already wraps a UUID; bounce through Uuid to stay generic.
        let raw_uuid: uuid::Uuid = ws.into();
        return Ok(T::from(raw_uuid));
    }

    // 3. Neither path. Mention both in the error.
    Err(ServiceError::Validation(format!(
        "missing workspace id: provide `{arg_name}` arg or `context.workspace_id`"
    )))
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
        .any(|g| g.emoji == emoji && g.user_ids.contains(&actor_user_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::server::model::ServerId;
    use rim_services::command::dto::CommandContextDto;

    fn ctx_with_workspace(ws: ServerId) -> CommandContextDto {
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

    fn ctx_without_workspace() -> CommandContextDto {
        CommandContextDto {
            workspace_id: None,
            conversation_id: None,
            thread_id: None,
            selected_text: None,
            selected_message_id: None,
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        }
    }

    fn args_with_server_id(server_id: ServerId) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        m.insert(
            "server_id".to_string(),
            serde_json::Value::String(server_id.to_string()),
        );
        m
    }

    fn empty_args() -> serde_json::Map<String, serde_json::Value> {
        serde_json::Map::new()
    }

    /// 1. Arg only — no context. Returns the arg UUID.
    #[test]
    fn resolve_workspace_id_arg_only_returns_arg_uuid() {
        let ws = ServerId::new();
        let args = args_with_server_id(ws);
        let result = resolve_workspace_id::<ServerId>(&args, None, "server_id");
        assert_eq!(result.unwrap(), ws);
    }

    /// 2. Context only — empty args. Returns the context UUID.
    #[test]
    fn resolve_workspace_id_context_only_returns_context_uuid() {
        let ws = ServerId::new();
        let ctx = ctx_with_workspace(ws);
        let result = resolve_workspace_id::<ServerId>(&empty_args(), Some(&ctx), "server_id");
        assert_eq!(result.unwrap(), ws);
    }

    /// 3. Both supplied with different UUIDs — arg wins.
    #[test]
    fn resolve_workspace_id_arg_wins_when_both_present() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        assert_ne!(arg_ws, ctx_ws);

        let args = args_with_server_id(arg_ws);
        let ctx = ctx_with_workspace(ctx_ws);
        let result = resolve_workspace_id::<ServerId>(&args, Some(&ctx), "server_id");
        assert_eq!(result.unwrap(), arg_ws);
    }

    /// 4. Arg present but empty string — falls back to context UUID.
    #[test]
    fn resolve_workspace_id_arg_present_but_empty_falls_back_to_context() {
        let ws = ServerId::new();
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(String::new()),
        );
        let ctx = ctx_with_workspace(ws);
        let result = resolve_workspace_id::<ServerId>(&args, Some(&ctx), "server_id");
        assert_eq!(result.unwrap(), ws);
    }

    /// 5. Arg present but whitespace-only — falls back to context UUID.
    #[test]
    fn resolve_workspace_id_arg_present_but_whitespace_falls_back_to_context() {
        let ws = ServerId::new();
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String("   ".to_string()),
        );
        let ctx = ctx_with_workspace(ws);
        let result = resolve_workspace_id::<ServerId>(&args, Some(&ctx), "server_id");
        assert_eq!(result.unwrap(), ws);
    }

    /// 6. Neither arg nor context — Validation error mentioning both paths.
    #[test]
    fn resolve_workspace_id_neither_present_returns_validation_with_both_paths_in_msg() {
        let result = resolve_workspace_id::<ServerId>(&empty_args(), None, "server_id");
        let err = result.unwrap_err();
        assert!(
            matches!(err, ServiceError::Validation(_)),
            "expected Validation, got: {err:?}"
        );
        if let ServiceError::Validation(msg) = err {
            assert!(
                msg.contains("server_id"),
                "error message must mention `server_id`; got: {msg}"
            );
            assert!(
                msg.contains("context.workspace_id"),
                "error message must mention `context.workspace_id`; got: {msg}"
            );
        }
    }

    /// 7. Malformed arg — hard Validation error, does NOT fall through to context.
    #[test]
    fn resolve_workspace_id_malformed_arg_returns_validation() {
        let ws = ServerId::new();
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String("not-a-uuid".to_string()),
        );
        let ctx = ctx_with_workspace(ws);
        let result = resolve_workspace_id::<ServerId>(&args, Some(&ctx), "server_id");
        let err = result.unwrap_err();
        assert!(
            matches!(err, ServiceError::Validation(_)),
            "expected Validation for malformed uuid, got: {err:?}"
        );
        if let ServiceError::Validation(msg) = err {
            assert!(
                msg.contains("invalid uuid"),
                "error message must mention `invalid uuid`; got: {msg}"
            );
        }
    }

    /// 8. Context present but workspace_id is None — falls through to Validation.
    #[test]
    fn resolve_workspace_id_context_with_no_workspace_id_falls_through_to_error() {
        let ctx = ctx_without_workspace();
        let result = resolve_workspace_id::<ServerId>(&empty_args(), Some(&ctx), "server_id");
        assert!(
            matches!(result, Err(ServiceError::Validation(_))),
            "expected Validation when context has no workspace_id, got: {result:?}"
        );
    }

    /// 9. Context is None but arg is present — returns the arg UUID.
    #[test]
    fn resolve_workspace_id_context_none_with_args_works() {
        let ws = ServerId::new();
        let args = args_with_server_id(ws);
        let result = resolve_workspace_id::<ServerId>(&args, None, "server_id");
        assert_eq!(result.unwrap(), ws);
    }
}
