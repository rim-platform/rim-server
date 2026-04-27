use serde_json::Value;

use rim_services::message::command::{DeleteMessageCmd, EditMessageCmd, SendMessageCmd};
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};

use super::{parse_uuid_arg, require_string_arg};

pub(super) async fn post_message(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
) -> ServiceResult<CommandResultDto> {
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
    let conversation_id =
        parse_uuid_arg::<rim_core::conversation::model::ConversationId>(&args, "conversation_id")?;
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
        confirmation_required: None,
    })
}

pub(super) async fn reply_to_selected_message(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
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
        confirmation_required: None,
    })
}

pub(super) async fn delete_message(
    services: &CommandServices,
    actor: Actor,
    _args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
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
        confirmation_required: None,
    })
}

pub(super) async fn edit_message(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
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
        confirmation_required: None,
    })
}
