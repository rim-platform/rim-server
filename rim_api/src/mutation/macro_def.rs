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
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MacroService>>()?;

        // Same H1 back-compat bridge as executeCommand. The legacy
        // `confirmed_dangerous: true` is no longer self-confirming —
        // the inner ExecuteCommandCmd's confirmation gate will issue a
        // per-step challenge when it hits a dangerous step.
        let proof = match input.confirmation {
            Some(c) => Some(c.into_proof()?),
            None => {
                if input.confirmed_dangerous == Some(true) {
                    tracing::warn!(
                        target: "audit",
                        event = "deprecated_confirmed_dangerous_bool",
                        macro_id = %input.macro_id.as_str(),
                        "client sent legacy confirmed_dangerous=true on executeMacro; ignoring (H1 closure).",
                    );
                }
                None
            }
        };

        let result = svc
            .execute_macro(ExecuteMacroCmd {
                actor: Actor::new(actor),
                macro_id: parse_uuid_id(&input.macro_id)?,
                command_context: input.command_context.map(TryInto::try_into).transpose()?,
                confirmation: proof,
                // Same back-compat default rule as `executeCommand` — `Macro`
                // here means "the macro execution itself"; nested
                // `ExecuteCommandCmd` calls produced by macro expansion will
                // override to whatever the macro service decides per
                // step (Phase 1: same value as the outer initiator,
                // Phase 4: see propagation rule in CommandInitiator docs).
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
    /// **Deprecated** — see `ExecuteCommandInput.confirmed_dangerous`.
    /// Removed in Phase 4.
    confirmed_dangerous: Option<bool>,
    /// Server-issued confirmation proof. Same shape and semantics as
    /// `ExecuteCommandInput.confirmation`. Phase 1 note: macros are an
    /// "all-or-nothing" confirmation surface — a single proof issued
    /// at macro-execute time covers the whole expansion. Phase 4 may
    /// add per-step proofs once the modal can render a step list.
    confirmation: Option<crate::types::ConfirmationProofInput>,
    /// Who triggered this macro. Same back-compat semantics as
    /// `ExecuteCommandInput.initiator`: optional, server-defaults to
    /// `PALETTE`.
    initiator: Option<CommandInitiator>,
}
