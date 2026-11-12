use std::fmt;

use rim_core::command::model::UserCommandId;

use crate::command::confirmation::ConfirmationChallengeDto;
use crate::command::sensitivity::CommandSensitivityDto;
use crate::command::verb::TypedVerb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSourceKindDto {
    BuiltIn,
    Plugin,
    Integration,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandExecutionTargetDto {
    Backend,
    DesktopLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSurfaceDto {
    Palette,
    ContextMenu,
    KeybindPicker,
    Ai,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandArgumentTypeDto {
    String,
    Integer,
    Float,
    Boolean,
    Enum,
    UserId,
    ConversationId,
    MessageId,
    ThreadId,
    WorkspaceId,
    SelectionText,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandArgumentDto {
    pub name: String,
    pub title: String,
    pub description: String,
    pub argument_type: CommandArgumentTypeDto,
    pub required: bool,
    pub default_value: Option<serde_json::Value>,
    pub allowed_values: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandContextDto {
    pub workspace_id: Option<rim_core::server::model::ServerId>,
    pub conversation_id: Option<rim_core::conversation::model::ConversationId>,
    pub thread_id: Option<rim_core::message_thread::model::MessageThreadId>,
    pub selected_text: Option<String>,
    pub selected_message_id: Option<rim_core::message::model::MessageId>,
    pub selected_message_ids: Vec<rim_core::message::model::MessageId>,
    pub focused_surface: Option<String>,
    pub active_panel: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandDefinitionDto {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub source_kind: CommandSourceKindDto,
    pub execution_target: CommandExecutionTargetDto,
    pub arguments: Vec<CommandArgumentDto>,
    pub required_context: Vec<String>,
    pub required_capabilities: Vec<String>,
    pub visible_in_palette: bool,
    pub visible_in_context_menus: bool,
    pub keybind_capable: bool,
    pub dangerous: bool,
    pub user_command_id: Option<UserCommandId>,

    // --- AI agent metadata ------------------------------------------------
    /// Optional natural-language description used by the AI Agent when it
    /// surfaces this command as a tool. Falls back to `description` when
    /// `None`. A command opts in to AI surfacing by setting
    /// `ai_callable: true` alongside a populated `nl_description`.
    ///
    /// Passed through [`crate::command::sanitize::sanitize_nl_text`] at
    /// registration time — any prompt-injection payload is rejected at the
    /// source, before the string can reach the model.
    pub nl_description: Option<String>,

    /// Short example phrases a user might speak to invoke this command
    /// (up to [`crate::command::sanitize::MAX_INTENT_SAMPLES_COUNT`]).
    /// Fed to the agent as training/routing hints; each sample is
    /// independently sanitized at registration time.
    pub intent_samples: Vec<String>,

    /// Explicit opt-in for AI exposure. Set `true` to surface this command
    /// to the LLM tool list; `false` keeps the command human-only. The flag
    /// is authoritative: a command is **not** reachable by the AI Agent
    /// unless this field is `true`, regardless of other metadata.
    pub ai_callable: bool,

    /// Sensitivity tier driving the confirmation policy and the client agent's
    /// mutating-call cap, and recorded on the audit line. See
    /// [`crate::command::sensitivity::CommandSensitivityDto`]. Orthogonal
    /// to `dangerous`: `dangerous: true` always requires confirmation
    /// regardless of tier.
    pub sensitivity: CommandSensitivityDto,

    /// Semantic verb class for this command. Stored in the audit log so
    /// queries can group by verb-class without string parsing. Defaults to
    /// [`TypedVerb::Other`] for user-defined and plugin commands.
    pub typed_verb: TypedVerb,

    /// Maximum calls per actor per 60-second sliding window. `None` means
    /// unlimited. Enforced by the rate-limit gate in `execute_command` AFTER
    /// capability + confirmation gates. Single-node: each process counts on its own.
    pub rate_limit_per_minute: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandResultStatusDto {
    Success,
    Error,
    /// The command is `dangerous: true` and the caller did not submit a
    /// matching [`crate::command::confirmation::ConfirmationProof`].
    /// `CommandResultDto::confirmation_required` carries the challenge
    /// the frontend must show.
    ConfirmationRequired,
}

impl CommandResultStatusDto {
    /// Stable, lowercase string used in audit log lines.
    /// Mirrors the `CommandInitiator::as_audit_str` shape — operators grep
    /// for `result_status=success` / `result_status=error` etc.
    pub fn as_audit_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::ConfirmationRequired => "confirmation_required",
        }
    }
}

impl fmt::Display for CommandResultStatusDto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_audit_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PipeStageResultDto {
    pub stage_index: u32,
    pub command_id: String,
    pub status: CommandResultStatusDto,
    pub message: String,
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandResultDto {
    pub status: CommandResultStatusDto,
    pub message: String,
    pub payload: Option<serde_json::Value>,
    pub follow_up_commands: Vec<String>,
    pub pipeline_stages: Option<Vec<PipeStageResultDto>>,
    /// Populated **iff** `status == ConfirmationRequired`. Carries the
    /// server-issued challenge the frontend must present to the user;
    /// the user-confirmed re-submission must include a matching
    /// [`crate::command::confirmation::ConfirmationProof::InBandToken`].
    pub confirmation_required: Option<ConfirmationChallengeDto>,
}

/// One step of a macro: the command it runs, the arguments fixed for it and the names of the
/// arguments the client asks for when the macro runs.
#[derive(Debug, Clone, PartialEq)]
pub struct UserCommandStepDto {
    pub command_id: String,
    pub args: Option<serde_json::Value>,
    /// Argument names the client asks the person for before the macro's first step runs.
    pub ask: Vec<String>,
}

/// One step of a macro as an update gives it: an omitted `ask` keeps what the stored step at the
/// same position asked for when that step names the same command.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateUserCommandStepDto {
    pub command_id: String,
    pub args: Option<serde_json::Value>,
    pub ask: Option<Vec<String>>,
}

impl From<UserCommandStepDto> for UpdateUserCommandStepDto {
    /// The step with its `ask` given, so an update asks for exactly those arguments.
    fn from(step: UserCommandStepDto) -> Self {
        Self {
            command_id: step.command_id,
            args: step.args,
            ask: Some(step.ask),
        }
    }
}

/// A macro, a person's own command, as its owner saved it.
#[derive(Debug, Clone, PartialEq)]
pub struct UserCommandDto {
    pub id: UserCommandId,
    pub command_key: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepDto>,
    pub is_enabled: bool,
    /// Whether the owner lets the AI agent run the macro.
    pub ai_callable: bool,
    /// How the macro is described to the AI agent; empty for none.
    pub nl_description: String,
    /// Example phrases for the AI agent.
    pub intent_samples: Vec<String>,
}

impl CommandDefinitionDto {
    /// Returns `true` when this command requires a `ConfirmationProof` given
    /// the supplied initiator.
    ///
    /// Policy:
    /// - `dangerous: true` always requires confirmation, regardless of initiator.
    /// - `sensitivity >= Mutating` AND `initiator.is_agentic()` requires
    ///   confirmation — agents must not silently mutate state.  Human initiators
    ///   (Palette, Keybind, ContextMenu, Cli, Macro) may execute Mutating
    ///   commands without a separate proof; their initiator IS the
    ///   human-in-the-loop signal.
    /// - Restricted tier requires confirmation by tier alone
    ///   (`requires_confirmation_by_tier()`), which covers both human and
    ///   agentic paths.
    ///
    /// This is the **single source of truth** for the confirmation gate
    /// predicate. `execute_command` consults it for every call, including each
    /// step a macro or pipeline runs, and the macro service consults it for
    /// every step before a run to decide whether the run asks for one
    /// confirmation up front, so the two cannot disagree.
    pub fn requires_confirmation_for_initiator(
        &self,
        initiator: super::initiator::CommandInitiator,
    ) -> bool {
        if self.dangerous {
            return true;
        }
        if self.sensitivity.requires_confirmation_by_tier() {
            return true;
        }
        // Agent-initiator policy: Mutating+ commands require confirmation when
        // the caller is agentic (Agent, Plugin).
        if initiator.is_agentic()
            && self.sensitivity >= super::sensitivity::CommandSensitivityDto::Mutating
        {
            return true;
        }
        false
    }
}

/// What the steps of a macro add up to, as the resolver derives them.
#[derive(Debug, Clone, PartialEq)]
pub struct UserCommandCompositionDto {
    /// `Backend` when the server runs every step and no step asks for an argument.
    pub execution_target: CommandExecutionTargetDto,
    pub required_context: Vec<String>,
    pub required_capabilities: Vec<String>,
    /// Whether every step's command may be named by a keybind.
    pub keybind_capable: bool,
    /// Whether any step is dangerous.
    pub dangerous: bool,
    pub step_sensitivities: Vec<CommandSensitivityDto>,
    /// Whether the AI agent may run a macro with these steps: each an AI-callable builtin the
    /// server runs, with every argument fixed.
    pub agent_runnable: bool,
}

impl UserCommandDto {
    /// The macro's definition with nothing known of its steps: run by the server, keybind-capable,
    /// `Safe`, and not callable by the AI agent.
    pub fn as_command_definition(&self) -> CommandDefinitionDto {
        self.as_command_definition_with_metadata(UserCommandCompositionDto {
            execution_target: CommandExecutionTargetDto::Backend,
            required_context: vec![],
            required_capabilities: vec![],
            keybind_capable: true,
            dangerous: false,
            step_sensitivities: vec![],
            agent_runnable: false,
        })
    }

    /// The macro's definition given what its steps add up to: in category `Macro`, its
    /// sensitivity the highest of its steps', callable by the AI agent when its owner allows it
    /// and its steps still let the agent run it, and described to the agent by its
    /// natural-language description, or its description when that is empty.
    pub fn as_command_definition_with_metadata(
        &self,
        composition: UserCommandCompositionDto,
    ) -> CommandDefinitionDto {
        let sensitivity = CommandSensitivityDto::effective(
            CommandSensitivityDto::from_dangerous_flag(composition.dangerous),
            &composition.step_sensitivities,
        );
        let nl_description = if self.nl_description.trim().is_empty() {
            self.description.clone()
        } else {
            self.nl_description.clone()
        };
        CommandDefinitionDto {
            id: self.command_key.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            category: "Macro".to_string(),
            source_kind: CommandSourceKindDto::User,
            execution_target: composition.execution_target,
            arguments: vec![],
            required_context: composition.required_context,
            required_capabilities: composition.required_capabilities,
            visible_in_palette: true,
            visible_in_context_menus: false,
            keybind_capable: composition.keybind_capable,
            dangerous: composition.dangerous,
            user_command_id: Some(self.id),
            nl_description: Some(nl_description).filter(|text| !text.is_empty()),
            intent_samples: self.intent_samples.clone(),
            ai_callable: self.ai_callable && composition.agent_runnable,
            sensitivity,
            typed_verb: TypedVerb::Other,
            rate_limit_per_minute: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::initiator::CommandInitiator;

    const ALL_INITIATORS: [CommandInitiator; 9] = [
        CommandInitiator::Palette,
        CommandInitiator::Agent,
        CommandInitiator::Macro,
        CommandInitiator::Keybind,
        CommandInitiator::ContextMenu,
        CommandInitiator::Plugin,
        CommandInitiator::Cli,
        CommandInitiator::Ui,
        CommandInitiator::Bot,
    ];

    fn definition(dangerous: bool, sensitivity: CommandSensitivityDto) -> CommandDefinitionDto {
        CommandDefinitionDto {
            id: "admin.kick".to_string(),
            title: "Kick".to_string(),
            description: String::new(),
            category: "Admin".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![],
            required_context: vec![],
            required_capabilities: vec![],
            visible_in_palette: true,
            visible_in_context_menus: false,
            keybind_capable: true,
            dangerous,
            user_command_id: None,
            nl_description: None,
            intent_samples: vec![],
            ai_callable: false,
            sensitivity,
            typed_verb: TypedVerb::Other,
            rate_limit_per_minute: None,
        }
    }

    /// A dangerous command needs a confirmation from every initiator whatever its tier,
    /// including the `Macro` initiator and a tier as low as `Mutating`.
    #[test]
    fn dangerous_command_requires_confirmation_from_every_initiator() {
        for sensitivity in [
            CommandSensitivityDto::Safe,
            CommandSensitivityDto::Mutating,
            CommandSensitivityDto::Restricted,
        ] {
            let def = definition(true, sensitivity);
            for initiator in ALL_INITIATORS {
                assert!(
                    def.requires_confirmation_for_initiator(initiator),
                    "dangerous {sensitivity:?} from {initiator:?}"
                );
            }
        }
    }

    /// A `Restricted` command needs a confirmation from every initiator; a `Mutating` one
    /// that is not dangerous needs one only from an agentic initiator; `Safe` and `Sensitive`
    /// never do.
    #[test]
    fn tier_decides_when_the_command_is_not_dangerous() {
        for initiator in ALL_INITIATORS {
            assert!(
                definition(false, CommandSensitivityDto::Restricted)
                    .requires_confirmation_for_initiator(initiator)
            );
            assert_eq!(
                definition(false, CommandSensitivityDto::Mutating)
                    .requires_confirmation_for_initiator(initiator),
                initiator.is_agentic(),
                "Mutating from {initiator:?}"
            );
            for sensitivity in [
                CommandSensitivityDto::Safe,
                CommandSensitivityDto::Sensitive,
            ] {
                assert!(
                    !definition(false, sensitivity).requires_confirmation_for_initiator(initiator),
                    "{sensitivity:?} from {initiator:?}"
                );
            }
        }
    }
}
