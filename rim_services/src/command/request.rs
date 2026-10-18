use crate::Actor;
use crate::command::confirmation::ConfirmationProof;
use crate::command::dto::{CommandContextDto, CommandSurfaceDto, UserCommandStepDto};
use crate::command::initiator::CommandInitiator;

#[derive(Debug, Clone)]
pub struct ListCommandsQuery {
    pub actor: Actor,
    pub context: Option<CommandContextDto>,
    pub surface: CommandSurfaceDto,
}

#[derive(Debug, Clone)]
pub struct GetCommandQuery {
    pub actor: Actor,
    pub command_id: String,
}

/// Looks up one of the actor's own user commands by its command id (`user.<key>`).
#[derive(Debug, Clone)]
pub struct GetUserCommandQuery {
    pub actor: Actor,
    pub command_id: String,
}

/// Lists the actor's own user commands, enabled or not.
#[derive(Debug, Clone)]
pub struct ListUserCommandsQuery {
    pub actor: Actor,
}

#[derive(Debug, Clone)]
pub struct ExecuteCommandCmd {
    pub actor: Actor,
    pub command_id: String,
    pub args: Option<serde_json::Value>,
    pub context: Option<CommandContextDto>,
    /// Server-issued, server-verified confirmation proof. `None` means the
    /// caller is not asserting any prior confirmation — the gate decides
    /// based on the command's `dangerous` flag whether to issue a fresh
    /// challenge (a `ConfirmationRequired` result carrying it) or to
    /// execute. The proof is bound to `(actor, command_id, args, context)`
    /// server-side, so it cannot confirm a different call.
    pub confirmation: Option<ConfirmationProof>,
    pub is_pipe: bool,
    /// Who issued this command. Drives the audit-log entry and the
    /// confirmation-policy decision. The GraphQL layer fills in `Palette` when
    /// the client sends none — see [`crate::command::initiator::CommandInitiator`]
    /// for the policy and the wire contract.
    ///
    /// **Macro / pipeline expansion preserves the outermost initiator** —
    /// a macro triggered by `Agent` keeps `Agent` on every nested
    /// `execute_command`, NOT `Macro`, so the agent confirmation rule applies
    /// to nested steps too.
    pub initiator: CommandInitiator,
}

#[derive(Debug, Clone)]
pub struct CreateUserCommandCmd {
    pub actor: Actor,
    pub command_key: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepDto>,
}

#[derive(Debug, Clone)]
pub struct UpdateUserCommandCmd {
    pub actor: Actor,
    pub command_id: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepDto>,
    pub is_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct DeleteUserCommandCmd {
    pub actor: Actor,
    pub command_id: String,
}
