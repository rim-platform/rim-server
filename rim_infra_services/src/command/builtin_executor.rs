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
