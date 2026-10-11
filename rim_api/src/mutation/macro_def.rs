use async_graphql::*;
use rim_services::Actor;
use rim_services::command::initiator::CommandInitiator as CommandInitiatorDto;
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::macro_def::dto::MacroStepDto;
use rim_services::macro_def::request::{
    CreateMacroCmd, DeleteMacroCmd, ExecuteMacroCmd, UpdateMacroCmd,
};
use rim_services::macro_def::service::MacroService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::features::require_initiator;
use crate::ids::parse_uuid_id;
use crate::types::{
    CommandContextInput, CommandInitiator, CommandResult, CommandSensitivity, MacroDef,
};

#[derive(Default)]
pub(crate) struct MacroMutations;

#[Object]
impl MacroMutations {
    // ── Macros ──────────────────────────────────────────────────────────

    async fn create_macro(&self, ctx: &Context<'_>, input: CreateMacroInput) -> Result<MacroDef> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MacroService>>()?;

        let result = svc
            .create_macro(CreateMacroCmd {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&input.server_id)?,
                name: input.name,
                description: input.description,
                steps: input
                    .steps
                    .into_iter()
                    .map(|s| MacroStepDto {
                        command_id: s.command_id,
                        args: s.args.map(|j| j.0),
                    })
                    .collect(),
                ai_callable: input.ai_callable,
                nl_description: input.nl_description,
                intent_samples: input.intent_samples,
                sensitivity: CommandSensitivityDto::from(input.sensitivity),
            })
            .await
            .map_err(map_service_error)?;

        Ok(result.into())
    }

    async fn update_macro(&self, ctx: &Context<'_>, input: UpdateMacroInput) -> Result<MacroDef> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MacroService>>()?;

        let result = svc
            .update_macro(UpdateMacroCmd {
                actor: Actor::new(actor),
                macro_id: parse_uuid_id(&input.macro_id)?,
                name: input.name,
                description: input.description,
                steps: input
                    .steps
                    .into_iter()
                    .map(|s| MacroStepDto {
                        command_id: s.command_id,
                        args: s.args.map(|j| j.0),
                    })
                    .collect(),
                ai_callable: input.ai_callable,
                nl_description: input.nl_description,
                intent_samples: input.intent_samples,
                sensitivity: CommandSensitivityDto::from(input.sensitivity),
            })
            .await
            .map_err(map_service_error)?;

        Ok(result.into())
    }

    async fn delete_macro(&self, ctx: &Context<'_>, macro_id: ID) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MacroService>>()?;

        svc.delete_macro(DeleteMacroCmd {
            actor: Actor::new(actor),
            macro_id: parse_uuid_id(&macro_id)?,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn execute_macro(
        &self,
        ctx: &Context<'_>,
        input: ExecuteMacroInput,
    ) -> Result<CommandResult> {
        require_initiator(ctx, input.initiator)?;
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MacroService>>()?;

        let result = svc
            .execute_macro(ExecuteMacroCmd {
                actor: Actor::new(actor),
                macro_id: parse_uuid_id(&input.macro_id)?,
                command_context: input.command_context.map(TryInto::try_into).transpose()?,
                confirmation: input.confirmation.map(|c| c.into_proof()),
                // A missing initiator counts as `Palette`, as in `executeCommand`. The macro
                // service runs every step with this same initiator.
                initiator: input
                    .initiator
                    .map(CommandInitiatorDto::from)
                    .unwrap_or_default(),
            })
            .await
            .map_err(map_service_error)?;

        Ok(result.into())
    }
}

#[derive(InputObject)]
struct CreateMacroInput {
    server_id: ID,
    name: String,
    description: Option<String>,
    steps: Vec<MacroStepInput>,
    #[graphql(default)]
    ai_callable: bool,
    #[graphql(default)]
    nl_description: String,
    #[graphql(default)]
    intent_samples: Vec<String>,
    #[graphql(default_with = "CommandSensitivity::Safe")]
    sensitivity: CommandSensitivity,
}

#[derive(InputObject)]
struct UpdateMacroInput {
    macro_id: ID,
    name: String,
    description: Option<String>,
    steps: Vec<MacroStepInput>,
    #[graphql(default)]
    ai_callable: bool,
    #[graphql(default)]
    nl_description: String,
    #[graphql(default)]
    intent_samples: Vec<String>,
    #[graphql(default_with = "CommandSensitivity::Safe")]
    sensitivity: CommandSensitivity,
}

#[derive(InputObject)]
struct MacroStepInput {
    command_id: String,
    args: Option<async_graphql::Json<serde_json::Value>>,
}

#[derive(InputObject)]
struct ExecuteMacroInput {
    macro_id: ID,
    command_context: Option<CommandContextInput>,
    /// Confirmation proof, as in `ExecuteCommandInput.confirmation`. One proof issued for the
    /// macro covers every step of it.
    confirmation: Option<crate::types::ConfirmationProofInput>,
    /// Who triggered this macro, as in `ExecuteCommandInput.initiator`: `PALETTE` when omitted.
    /// Every step runs with this initiator.
    initiator: Option<CommandInitiator>,
}
