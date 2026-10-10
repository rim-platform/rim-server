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
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;

        // ── H1 back-compat bridge ──────────────────────────────────────
        //
        // The pre-reframe wire field `confirmed_dangerous: Option<bool>`
        // is still on the input for back-compat with the unmigrated
        // frontend. Resolution:
        //
        // 1. If the new `confirmation` field is populated, use it
        //    (modern flow — frontend has the modal).
        // 2. Else if `confirmed_dangerous: Some(true)` AND the new field
        //    is absent, log a deprecation warning. The proof stays
        //    `None` here; the service-layer gate will issue a fresh
        //    challenge and return `CONFIRMATION_REQUIRED`. Pre-Phase-1
        //    behavior (silent execute on bool=true) is intentionally
        //    NOT preserved — passing a bool used to be the bypass; we
        //    are closing it on purpose.
        // 3. Both `None` ⇒ no proof; gate issues a challenge.
        //
        // Removed entirely in Phase 4 when the frontend modal lands.
        let proof = match input.confirmation {
            Some(c) => Some(c.into_proof()?),
            None => {
                if input.confirmed_dangerous == Some(true) {
                    tracing::warn!(
                        target: "audit",
                        event = "deprecated_confirmed_dangerous_bool",
                        command_id = %input.command_id,
                        "client sent legacy confirmed_dangerous=true; ignoring (H1 closure). \
                         Frontend must migrate to ConfirmationProof flow.",
                    );
                }
                None
            }
        };

        let result = svc
            .execute_command(ExecuteCommandCmd {
                actor: Actor::new(actor),
                command_id: input.command_id,
                args: input.args.map(|json| json.0),
                context: input.command_context.map(TryInto::try_into).transpose()?,
                confirmation: proof,
                is_pipe: input.is_pipe.unwrap_or(false),
                // Default to Palette per Fury v2 §3.11 conservative-default
                // rule. Until Phase 4's IPC proxy stamps the initiator from
                // the webview origin, this is the only safe default.
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
    /// **Deprecated** — use `confirmation` instead. Pre-Phase-1 callers
    /// (the unmigrated frontend) set this to `true` after `window.confirm()`.
    /// When `confirmation` is absent and this is `Some(true)`, the server
    /// synthesizes a self-issued in-band proof so the H1 gate has a single
    /// authoritative path. Removed in Phase 4 once the frontend modal lands.
    pub confirmed_dangerous: Option<bool>,
    /// Server-issued, server-verified confirmation proof. Required when
    /// re-submitting after a previous `executeCommand` returned
    /// `status: CONFIRMATION_REQUIRED`. Closes H1 (TOCTOU) by binding the
    /// proof to `(actor, commandId, args, context)` server-side.
    pub confirmation: Option<crate::types::ConfirmationProofInput>,
    pub is_pipe: Option<bool>,
    /// Who issued this command. Optional for back-compat — when omitted, the
    /// backend defaults to `PALETTE` (the conservative human default). Phase
    /// 4 frontend dispatcher will start sending the explicit value from every
    /// call site (palette / agent / keybind / context-menu / macro). Until
    /// then this field is opt-in.
    ///
    /// Per Fury v2 §3.11. The default is enforced server-side
    /// (`CommandInitiator::default() == Palette`) so a client that submits
    /// `null` is treated as a human action — never as agent action.
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
