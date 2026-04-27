use serde_json::Value;

use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandResultDto, CommandResultStatusDto};

use super::parse_uuid_arg;

pub(super) async fn invite_member(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
) -> ServiceResult<CommandResultDto> {
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;

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
        confirmation_required: None,
    })
}
