use async_graphql::*;
use rim_services::Actor;
use rim_services::command::dto::UserCommandStepDto;
use rim_services::command::initiator::CommandInitiator as CommandInitiatorDto;
use rim_services::command::request::{
    CreateUserCommandCmd, DeleteUserCommandCmd, ExecuteCommandCmd, UpdateUserCommandCmd,
};
use rim_services::command::service::CommandService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
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
            })
            .await
            .map_err(map_service_error)?;

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
                    })
                    .collect(),
            })
            .await
            .map_err(map_service_error)?;

        Ok(command.as_command_definition().into())
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
                    .map(|step| UserCommandStepDto {
                        command_id: step.command_id,
                        args: step.args.map(|json| json.0),
                    })
                    .collect(),
                is_enabled: input.is_enabled,
            })
            .await
            .map_err(map_service_error)?;

        Ok(command.as_command_definition().into())
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
}

#[derive(InputObject)]
pub struct UserCommandStepInput {
    pub command_id: String,
    pub args: Option<async_graphql::Json<serde_json::Value>>,
}

#[derive(InputObject)]
pub struct CreateUserCommandInput {
    pub command_id: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepInput>,
}

#[derive(InputObject)]
pub struct UpdateUserCommandInput {
    pub command_id: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepInput>,
    pub is_enabled: bool,
}

#[derive(InputObject)]
pub struct DeleteUserCommandInput {
    pub command_id: String,
}
