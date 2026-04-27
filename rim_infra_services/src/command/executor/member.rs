use serde_json::Value;

use rim_services::server_member::command::ListServerMembersQuery;
use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandResultDto, CommandResultStatusDto};

use super::parse_uuid_arg;

pub(super) async fn list_members(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
) -> ServiceResult<CommandResultDto> {
    // P4-A5: thin adapter over ServerMemberService::list_members.
    // Projects to `{members: [{username, role, joined}]}` ONLY.
    // Widow no-go: do NOT leak `email`, `user_id`, `avatar_url`,
    // `display_name`, `bio`, `status_text`. A future regression
    // that passes the ServerMemberDto through serde directly
    // would re-widen PII — the explicit map below is the gate.
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;

    let members = services
        .server_member_service
        .list_members(ListServerMembersQuery { actor, server_id })
        .await?;

    let projected: Vec<Value> = members
        .iter()
        .map(|m| {
            serde_json::json!({
                "username": m.username,
                "role": m.member_role,
                "joined": m.joined_at.to_rfc3339(),
            })
        })
        .collect();

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: format!("{} members found", projected.len()),
        payload: Some(serde_json::json!({ "members": projected })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}
