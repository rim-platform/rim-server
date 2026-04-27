use serde_json::Value;

use rim_services::pin::command::PinMessageCmd;
use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandResultDto, CommandResultStatusDto};

use super::parse_uuid_arg;

pub(super) async fn pin_message(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
) -> ServiceResult<CommandResultDto> {
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
    let conversation_id =
        parse_uuid_arg::<rim_core::conversation::model::ConversationId>(&args, "conversation_id")?;
    let message_id = parse_uuid_arg::<rim_core::message::model::MessageId>(&args, "message_id")?;

    services
        .pin_service
        .pin_message(PinMessageCmd {
            actor,
            server_id,
            conversation_id,
            message_id,
        })
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "message pinned".to_string(),
        payload: Some(serde_json::json!({
            "success": true,
            "pinned": message_id.to_string(),
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}
