use crate::Actor;
use crate::command::confirmation::ConfirmationProof;
use crate::command::dto::{CommandContextDto, CommandSurfaceDto, UserCommandStepDto};
use crate::command::initiator::CommandInitiator;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ListCommandsQuery {
    pub actor: Actor,
    pub context: Option<CommandContextDto>,
    pub surface: CommandSurfaceDto,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GetCommandQuery {
    pub actor: Actor,
    pub command_id: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ExecuteCommandCmd {
    pub actor: Actor,
    pub command_id: String,
    pub args: Option<serde_json::Value>,
    pub context: Option<CommandContextDto>,
    /// Server-issued, server-verified confirmation proof. `None` means the
    /// caller is not asserting any prior confirmation — the gate decides
    /// based on the command's `dangerous` flag whether to issue a fresh
    /// challenge (returning [`crate::command::confirmation::ConfirmationFlowDto::ConfirmationRequired`])
    /// or to execute. Closes H1 (TOCTOU) by binding the proof to
    /// `(actor, command_id, args, context)` server-side.
    ///
    /// Replaces the pre-Phase-1 `confirmed_dangerous: bool` flag. The
    /// GraphQL boundary keeps a deprecated `confirmed_dangerous`
    /// back-compat alias until the frontend modal lands in Phase 4.
    pub confirmation: Option<ConfirmationProof>,
    pub is_pipe: bool,
    /// Who issued this command. Drives the audit-log entry, the
    /// confirmation-policy decision, and (Phase 4) the agent's per-turn
    /// mutating-call cap. Defaults to `Palette` for back-compat with any
    /// caller that hasn't been retrofitted yet — see
    /// [`crate::command::initiator::CommandInitiator`] for the policy
    /// rationale and the wire contract.
    ///
    /// **Macro / pipeline expansion preserves the outermost initiator** —
    /// a macro triggered by `Agent` keeps `Agent` on every nested
    /// `execute_command`, NOT `Macro`. The dispatch policy reads this field
    /// to decide whether the agent's rate cap applies to nested steps.
    pub initiator: CommandInitiator,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CreateUserCommandCmd {
    pub actor: Actor,
    pub command_key: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepDto>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UpdateUserCommandCmd {
    pub actor: Actor,
    pub command_id: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepDto>,
    pub is_enabled: bool,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeleteUserCommandCmd {
    pub actor: Actor,
    pub command_id: String,
}
