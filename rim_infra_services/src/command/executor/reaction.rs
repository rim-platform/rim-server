use serde_json::Value;

use rim_services::message_reaction::command::ToggleReactionCmd;
use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandResultDto, CommandResultStatusDto};

use super::{parse_uuid_arg, reaction_has_actor, require_string_arg};

pub(super) async fn toggle_reaction(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
) -> ServiceResult<CommandResultDto> {
    // P4-A5: name is `toggle_reaction` (Tony's call) — backend
    // mutation is bidirectional. `action` in the payload tells the
    // LLM whether the call added or removed the reaction, so a
    // subsequent turn doesn't blindly retry "add" when it meant to.
    // Detection: compare reaction groups pre/post for the actor's
    // user id + emoji. Simpler than reading the return type's
    // semantics directly since the toggle response is just the
    // final grouped state.
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
    let conversation_id =
        parse_uuid_arg::<rim_core::conversation::model::ConversationId>(&args, "conversation_id")?;
    let message_id = parse_uuid_arg::<rim_core::message::model::MessageId>(&args, "message_id")?;
    let emoji = require_string_arg(&args, "emoji")?;

    let actor_user_id = actor.user_id;

    let before = services
        .message_reaction_service
        .get_reactions(
            rim_services::message_reaction::command::GetMessageReactionsQuery {
                actor,
                server_id,
                conversation_id,
                message_id,
            },
        )
        .await?;
    let had_reaction = reaction_has_actor(&before, &emoji, actor_user_id);

    services
        .message_reaction_service
        .toggle_reaction(ToggleReactionCmd {
            actor,
            server_id,
            conversation_id,
            message_id,
            emoji: emoji.clone(),
        })
        .await?;

    let action = if had_reaction { "removed" } else { "added" };

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: format!("reaction {action}"),
        payload: Some(serde_json::json!({
            "success": true,
            "toggled": message_id.to_string(),
            "emoji": emoji,
            "action": action,
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}
