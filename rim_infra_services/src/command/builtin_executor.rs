use serde_json::Value;

use rim_services::conversation::command::CreateConversationCmd;
use rim_services::conversation::types::ConversationParticipants;
use rim_services::message::command::{DeleteMessageCmd, EditMessageCmd, SendMessageCmd};
use rim_services::{Actor, ServiceError, ServiceResult};

use super::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};

fn require_object_args(args: Option<Value>) -> ServiceResult<serde_json::Map<String, Value>> {
    match args {
        Some(Value::Object(map)) => Ok(map),
        Some(_) => Err(ServiceError::Validation(
            "command args must be a JSON object".to_string(),
        )),
        None => Ok(serde_json::Map::new()),
    }
}

fn require_string_arg(map: &serde_json::Map<String, Value>, key: &str) -> ServiceResult<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ServiceError::Validation(format!("missing string arg `{key}`")))
}

fn optional_string_arg(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(|value| value.to_string())
}

fn parse_uuid_arg<T>(map: &serde_json::Map<String, Value>, key: &str) -> ServiceResult<T>
where
    T: From<uuid::Uuid>,
{
    let raw = require_string_arg(map, key)?;
    let uuid = uuid::Uuid::parse_str(&raw)
        .map_err(|_| ServiceError::Validation(format!("invalid uuid arg `{key}`")))?;
    Ok(T::from(uuid))
}

fn parse_uuid_array_arg<T>(map: &serde_json::Map<String, Value>, key: &str) -> ServiceResult<Vec<T>>
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

pub(super) async fn execute_builtin_command(
    services: &CommandServices,
    actor: Actor,
    command_id: &str,
    args: Option<Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let args = require_object_args(args)?;

    match command_id {
        "chat.post_message" => {
            let server_id =
                parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
            let conversation_id = parse_uuid_arg::<rim_core::conversation::model::ConversationId>(
                &args,
                "conversation_id",
            )?;
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
            })
        }
        "chat.create_group" => {
            let server_id =
                parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
            let member_ids =
                parse_uuid_array_arg::<rim_core::user::model::UserId>(&args, "member_ids")?;
            let conversation = services
                .conversation_service
                .create_conversation(CreateConversationCmd {
                    actor,
                    server_id,
                    participants: ConversationParticipants::Group { member_ids },
                    title: optional_string_arg(&args, "title"),
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
            })
        }
        "chat.reply_to_selected_message" => {
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
            })
        }
        "chat.delete_message" => {
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
            })
        }
        "chat.edit_message" => {
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
            })
        }
        "chat.create_channel" => {
            let server_id =
                parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
            let name = require_string_arg(&args, "name")?;

            let conversation = services
                .conversation_service
                .create_conversation(CreateConversationCmd {
                    actor,
                    server_id,
                    participants: ConversationParticipants::Channel,
                    title: Some(name),
                })
                .await?;

            Ok(CommandResultDto {
                status: CommandResultStatusDto::Success,
                message: "channel created".to_string(),
                payload: Some(serde_json::json!({
                    "conversation_id": conversation.id.to_string(),
                })),
                follow_up_commands: vec![],
                pipeline_stages: None,
            })
        }
        "admin.invite_member" => {
            let server_id =
                parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;

            let result = services
                .auth_service
                .create_invite(rim_services::auth::command::CreateInviteCmd {
                    actor,
                    server_id,
                    expires_at: None,
                    max_uses: 1,
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
            })
        }
        "chat.mark_as_read" => {
            let server_id =
                parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
            let conversation_id = parse_uuid_arg::<rim_core::conversation::model::ConversationId>(
                &args,
                "conversation_id",
            )?;

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
            })
        }
        "chat.mark_all_as_read" => {
            let server_id =
                parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;

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
            })
        }
        "state.get" => {
            let server_id =
                parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
            let key = require_string_arg(&args, "key")?;
            let scope = parse_scope_arg(&args)?;

            let entry = services
                .workspace_state_service
                .get(
                    rim_services::workspace_state::request::GetWorkspaceStateQuery {
                        actor,
                        server_id,
                        key,
                        scope,
                    },
                )
                .await?;

            Ok(CommandResultDto {
                status: CommandResultStatusDto::Success,
                message: "state retrieved".to_string(),
                payload: entry.map(|e| {
                    let scope_str = match e.scope {
                        rim_services::workspace_state::dto::WorkspaceStateScopeDto::Server => {
                            "server"
                        }
                        rim_services::workspace_state::dto::WorkspaceStateScopeDto::User => "user",
                    };
                    serde_json::json!({
                        "key": e.key,
                        "value": e.value,
                        "version": e.version,
                        "scope": scope_str,
                    })
                }),
                follow_up_commands: vec![],
                pipeline_stages: None,
            })
        }
        "state.set" => {
            let server_id =
                parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
            let key = require_string_arg(&args, "key")?;
            let value_str = require_string_arg(&args, "value")?;
            let value: serde_json::Value = match serde_json::from_str(&value_str) {
                Ok(v) => v,
                Err(_) => serde_json::Value::String(value_str),
            };
            let scope = parse_scope_arg(&args)?;
            let ttl_seconds = optional_string_arg(&args, "ttl").and_then(|s| s.parse::<i64>().ok());

            let entry = services
                .workspace_state_service
                .set(
                    rim_services::workspace_state::request::SetWorkspaceStateCmd {
                        actor,
                        server_id,
                        key,
                        value,
                        scope,
                        ttl_seconds,
                    },
                )
                .await?;

            Ok(CommandResultDto {
                status: CommandResultStatusDto::Success,
                message: "state set".to_string(),
                payload: Some(serde_json::json!({
                    "key": entry.key,
                    "value": entry.value,
                    "version": entry.version,
                })),
                follow_up_commands: vec![],
                pipeline_stages: None,
            })
        }
        "state.delete" => {
            let server_id =
                parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
            let key = require_string_arg(&args, "key")?;
            let scope = parse_scope_arg(&args)?;

            let deleted = services
                .workspace_state_service
                .delete(
                    rim_services::workspace_state::request::DeleteWorkspaceStateCmd {
                        actor,
                        server_id,
                        key: key.clone(),
                        scope,
                    },
                )
                .await?;

            Ok(CommandResultDto {
                status: CommandResultStatusDto::Success,
                message: if deleted {
                    "state deleted".to_string()
                } else {
                    "state key not found".to_string()
                },
                payload: Some(serde_json::json!({
                    "key": key,
                    "deleted": deleted,
                })),
                follow_up_commands: vec![],
                pipeline_stages: None,
            })
        }
        "state.list" => {
            let server_id =
                parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
            let prefix = optional_string_arg(&args, "prefix").unwrap_or_default();
            let scope = parse_scope_arg(&args)?;
            let limit = optional_string_arg(&args, "limit").and_then(|s| s.parse::<u64>().ok());

            let entries = services
                .workspace_state_service
                .list(
                    rim_services::workspace_state::request::ListWorkspaceStateQuery {
                        actor,
                        server_id,
                        prefix,
                        scope,
                        limit,
                    },
                )
                .await?;

            let items: Vec<serde_json::Value> = entries
                .into_iter()
                .map(|e| {
                    serde_json::json!({
                        "key": e.key,
                        "value": e.value,
                        "version": e.version,
                    })
                })
                .collect();

            Ok(CommandResultDto {
                status: CommandResultStatusDto::Success,
                message: format!("{} entries found", items.len()),
                payload: Some(serde_json::json!({ "entries": items })),
                follow_up_commands: vec![],
                pipeline_stages: None,
            })
        }
        "state.incr" => {
            let server_id =
                parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
            let key = require_string_arg(&args, "key")?;
            let delta = optional_string_arg(&args, "delta")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(1);
            let scope = parse_scope_arg(&args)?;

            let entry = services
                .workspace_state_service
                .increment(
                    rim_services::workspace_state::request::IncrementWorkspaceStateCmd {
                        actor,
                        server_id,
                        key,
                        scope,
                        delta,
                    },
                )
                .await?;

            Ok(CommandResultDto {
                status: CommandResultStatusDto::Success,
                message: "state incremented".to_string(),
                payload: Some(serde_json::json!({
                    "key": entry.key,
                    "value": entry.value,
                    "version": entry.version,
                })),
                follow_up_commands: vec![],
                pipeline_stages: None,
            })
        }
        "workspace.open_command_palette"
        | "workspace.cycle_recent_activity"
        | "chat.copy_message_text"
        | "chat.open_thread" => Err(ServiceError::Validation(
            "desktop-local commands must run in the client runtime".to_string(),
        )),
        _ => Err(ServiceError::NotFound),
    }
}

fn parse_scope_arg(
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
