use async_graphql::*;
use rim_services::command::dto::{UpdateUserCommandStepDto, UserCommandDto, UserCommandStepDto};
use rim_services::command::initiator::CommandInitiator as CommandInitiatorDto;
use rim_services::command::request::{
    CreateUserCommandCmd, DeleteUserCommandCmd, ExecuteCommandCmd, GetCommandQuery,
    UpdateUserCommandCmd,
};
use rim_services::command::service::CommandService;
use rim_services::{Actor, ServiceError};

use crate::auth_context::require_actor;
use crate::errors::{map_command_error, map_service_error};
use crate::features::require_initiator;
use crate::types::{Command, CommandContextInput, CommandInitiator, CommandResult};

#[derive(Default)]
pub(crate) struct CommandMutations;

#[Object]
impl CommandMutations {
    async fn execute_command(
        &self,
        ctx: &Context<'_>,
        input: ExecuteCommandInput,
    ) -> Result<CommandResult> {
        require_initiator(ctx, input.initiator)?;
        let actor = require_actor(ctx)?;
        if let Some(run_id) = &input.run_id {
            validate_run_id(run_id).map_err(map_service_error)?;
        }

        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;

        let result = svc
            .execute_command(ExecuteCommandCmd {
                actor: Actor::new(actor),
                command_id: input.command_id,
                args: input.args.map(|json| json.0),
                context: input.command_context.map(TryInto::try_into).transpose()?,
                confirmation: input.confirmation.map(|c| c.into_proof()),
                is_pipe: input.is_pipe.unwrap_or(false),
                // A missing initiator counts as a human palette call, never as an agent.
                initiator: input
                    .initiator
                    .map(CommandInitiatorDto::from)
                    .unwrap_or_default(),
                run_id: input.run_id,
            })
            .await
            .map_err(map_command_error)?;

        Ok(result.into())
    }

    async fn create_user_command(
        &self,
        ctx: &Context<'_>,
        input: CreateUserCommandInput,
    ) -> Result<Command> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;
        let command = svc
            .create_user_command(CreateUserCommandCmd {
                actor: Actor::new(actor),
                command_key: input.command_id,
                title: input.title,
                description: input.description,
                steps: input
                    .steps
                    .into_iter()
                    .map(|step| UserCommandStepDto {
                        command_id: step.command_id,
                        args: step.args.map(|json| json.0),
                        ask: step.ask.unwrap_or_default(),
                    })
                    .collect(),
                ai_callable: input.ai_callable,
                nl_description: input.nl_description.unwrap_or_default(),
                intent_samples: input.intent_samples.unwrap_or_default(),
            })
            .await
            .map_err(map_service_error)?;

        resolved_definition(svc.as_ref(), Actor::new(actor), command).await
    }

    async fn update_user_command(
        &self,
        ctx: &Context<'_>,
        input: UpdateUserCommandInput,
    ) -> Result<Command> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;
        let command = svc
            .update_user_command(UpdateUserCommandCmd {
                actor: Actor::new(actor),
                command_id: input.command_id,
                title: input.title,
                description: input.description,
                steps: input
                    .steps
                    .into_iter()
                    .map(|step| UpdateUserCommandStepDto {
                        command_id: step.command_id,
                        args: step.args.map(|json| json.0),
                        ask: step.ask,
                    })
                    .collect(),
                is_enabled: input.is_enabled,
                ai_callable: input.ai_callable,
                nl_description: input.nl_description,
                intent_samples: input.intent_samples,
            })
            .await
            .map_err(map_service_error)?;

        resolved_definition(svc.as_ref(), Actor::new(actor), command).await
    }

    async fn delete_user_command(
        &self,
        ctx: &Context<'_>,
        input: DeleteUserCommandInput,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;
        svc.delete_user_command(DeleteUserCommandCmd {
            actor: Actor::new(actor),
            command_id: input.command_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}

#[derive(InputObject)]
pub struct ExecuteCommandInput {
    pub command_id: String,
    pub args: Option<async_graphql::Json<serde_json::Value>>,
    pub command_context: Option<CommandContextInput>,
    /// Confirmation proof from a previous `executeCommand` that returned
    /// `status: CONFIRMATION_REQUIRED`. Required when re-submitting that call. The server
    /// binds the proof to the actor, command id, arguments and context, so it cannot confirm a
    /// different call.
    pub confirmation: Option<crate::types::ConfirmationProofInput>,
    pub is_pipe: Option<bool>,
    /// Who issued this command. When omitted, the server treats the call as `PALETTE`, a
    /// human action, never as an agent action. `AGENT` fails with `FEATURE_DISABLED` while the
    /// instance has the AI command surface off.
    pub initiator: Option<CommandInitiator>,
    /// An identifier the desktop app gives one stretch of an agent's calls. Kept on the audit
    /// entry when the initiator is `AGENT`, and ignored otherwise.
    pub run_id: Option<String>,
}

/// Longest `runId` `executeCommand` accepts.
const MAX_RUN_ID_LEN: usize = 64;

/// Checks that a `runId` is 1 to 64 ASCII letters, digits, `_` or `-`.
///
/// # Errors
///
/// [`ServiceError::Validation`] when it is not.
fn validate_run_id(run_id: &str) -> Result<(), ServiceError> {
    let allowed = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    if run_id.is_empty() || run_id.len() > MAX_RUN_ID_LEN || !run_id.bytes().all(allowed) {
        return Err(ServiceError::Validation(format!(
            "runId must be 1 to {MAX_RUN_ID_LEN} letters, digits, underscores or hyphens"
        )));
    }
    Ok(())
}

/// One step of a macro.
#[derive(InputObject)]
pub struct UserCommandStepInput {
    /// A built-in, app or plugin command id.
    pub command_id: String,
    pub args: Option<async_graphql::Json<serde_json::Value>>,
    /// Names of the arguments the client asks for when the macro runs: at most 32, each 1 to
    /// 64 bytes, none twice and none a key of `args`. Omitted, none on create, and on update
    /// what the stored step at the same position asked for when it names the same command.
    pub ask: Option<Vec<String>>,
}

#[derive(InputObject)]
pub struct CreateUserCommandInput {
    pub command_id: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepInput>,
    /// Lets the AI agent run the macro; refused unless every step is an AI-callable built-in
    /// the server runs, with every argument fixed.
    #[graphql(default = false)]
    pub ai_callable: bool,
    /// How the macro is described to the AI agent, at most 1,000 characters.
    pub nl_description: Option<String>,
    /// Up to 10 example phrases for the AI agent, each at most 200 characters.
    pub intent_samples: Option<Vec<String>>,
}

#[derive(InputObject)]
pub struct UpdateUserCommandInput {
    pub command_id: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepInput>,
    pub is_enabled: bool,
    /// Omitted, keeps the stored value, which turns off when the new steps no longer let the
    /// agent run the macro.
    pub ai_callable: Option<bool>,
    /// Omitted, keeps the stored value.
    pub nl_description: Option<String>,
    /// Omitted, keeps the stored value.
    pub intent_samples: Option<Vec<String>>,
}

#[derive(InputObject)]
pub struct DeleteUserCommandInput {
    pub command_id: String,
}

/// `command` as the `commands` query resolves it, so the returned tier reflects its steps; a
/// disabled command does not resolve and falls back to its stored fields.
async fn resolved_definition(
    svc: &dyn CommandService,
    actor: Actor,
    command: UserCommandDto,
) -> Result<Command> {
    let resolved = svc
        .get_command(GetCommandQuery {
            actor,
            command_id: command.command_key.clone(),
        })
        .await
        .map_err(map_service_error)?;
    Ok(resolved
        .unwrap_or_else(|| command.as_command_definition())
        .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_are_short_and_plain() {
        let longest = "x".repeat(MAX_RUN_ID_LEN);
        for good in ["r_1", "A-b_9", longest.as_str()] {
            assert!(validate_run_id(good).is_ok(), "{good}");
        }
        let too_long = "x".repeat(MAX_RUN_ID_LEN + 1);
        for bad in ["", "has space", "r/1", "r\n", "\u{e9}", too_long.as_str()] {
            assert!(
                matches!(validate_run_id(bad), Err(ServiceError::Validation(_))),
                "{bad:?}"
            );
        }
    }
}
