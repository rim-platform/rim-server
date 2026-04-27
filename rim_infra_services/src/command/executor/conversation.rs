use serde_json::Value;

use rim_services::conversation::command::CreateConversationCmd;
use rim_services::conversation::types::ConversationParticipants;
use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandResultDto, CommandResultStatusDto};

use super::{optional_string_arg, parse_uuid_arg, parse_uuid_array_arg};

pub(super) async fn create_group(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
) -> ServiceResult<CommandResultDto> {
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
    let member_ids = parse_uuid_array_arg::<rim_core::user::model::UserId>(&args, "member_ids")?;
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
        confirmation_required: None,
    })
}
