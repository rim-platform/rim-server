use serde_json::Value;

use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandResultDto, CommandResultStatusDto};

use super::{optional_string_arg, parse_scope_arg, parse_uuid_arg, require_string_arg};

pub(super) async fn get(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
) -> ServiceResult<CommandResultDto> {
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
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
                rim_services::workspace_state::dto::WorkspaceStateScopeDto::Server => "server",
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
        confirmation_required: None,
    })
}

pub(super) async fn set(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
) -> ServiceResult<CommandResultDto> {
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
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
        confirmation_required: None,
    })
}

pub(super) async fn delete(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
) -> ServiceResult<CommandResultDto> {
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
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
        confirmation_required: None,
    })
}

pub(super) async fn list(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
) -> ServiceResult<CommandResultDto> {
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
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
        confirmation_required: None,
    })
}

pub(super) async fn incr(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
) -> ServiceResult<CommandResultDto> {
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
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
        confirmation_required: None,
    })
}
