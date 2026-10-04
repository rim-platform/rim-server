use std::fmt;

use rim_core::command::model::UserCommandId;
use rim_core::workspace_feature::model::WorkspaceFeature;

use crate::command::confirmation::ConfirmationChallengeDto;
use crate::command::sensitivity::CommandSensitivityDto;
use crate::command::verb::TypedVerb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CommandSourceKindDto {
    BuiltIn,
    Plugin,
    Integration,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CommandExecutionTargetDto {
    Backend,
    DesktopLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CommandSurfaceDto {
    Palette,
    ContextMenu,
    KeybindPicker,
    Ai,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

    // --- AI Agent reframe Phase 1 (§3.1) ----------------------------------
    /// Optional natural-language description used by the AI Agent when it
    /// surfaces this command as a tool. Falls back to `description` when
    /// `None` — at Phase 1 rollout every built-in sets this to `None` and
    /// non-AI-callable commands will continue to use the palette `description`
    /// verbatim. A command opts in to AI surfacing by setting
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

    /// Sensitivity tier used for audit verbosity, mutating-call caps, and
    /// default confirmation/modal placement. See
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
    /// capability + confirmation gates. Single-node only in Phase 5.
    pub rate_limit_per_minute: Option<u32>,

    /// Track 0.4: the optional workspace module this command belongs to. `None` for commands
    /// that are always available (every non-incident builtin, user commands, macros, plugins).
    /// When `Some`, `list_commands` hides the command and `execute_command` rejects it with
    /// `ServiceError::FeatureDisabled` unless the feature is enabled for the context's
    /// workspace; without a workspace context the command is hidden and rejected.
    pub feature: Option<WorkspaceFeature>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CommandResultStatusDto {
    Success,
    Error,
    /// The command is `dangerous: true` and the caller did not submit a
    /// matching [`crate::command::confirmation::ConfirmationProof`].
    /// `CommandResultDto::confirmation_required` carries the challenge
    /// the frontend must show. Closes H1.
    ConfirmationRequired,
}

impl CommandResultStatusDto {
    /// Stable, lowercase string used in audit log lines (Phase 4 P4-A1).
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PipeStageResultDto {
    pub stage_index: u32,
    pub command_id: String,
    pub status: CommandResultStatusDto,
    pub message: String,
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UserCommandStepDto {
    pub command_id: String,
    pub args: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UserCommandDto {
    pub id: UserCommandId,
    pub command_key: String,
    pub title: String,
    pub description: String,
    pub steps: Vec<UserCommandStepDto>,
    pub is_enabled: bool,
}

impl CommandDefinitionDto {
    /// Returns `true` when this command requires a `ConfirmationProof` given
    /// the supplied initiator.
    ///
    /// Policy (P6-M2, Fury §3.5 + §3.11):
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
    /// predicate.  `execute_command` and `execute_macro` both consult this
    /// helper so the policy cannot drift.
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
        // the caller is agentic (Agent, Plugin, Mcp).
        if initiator.is_agentic()
            && self.sensitivity >= super::sensitivity::CommandSensitivityDto::Mutating
        {
            return true;
        }
        false
    }
}

impl UserCommandDto {
    pub fn as_command_definition(&self) -> CommandDefinitionDto {
        self.as_command_definition_with_metadata(
            CommandExecutionTargetDto::Backend,
            vec![],
            vec![],
            true,
            false,
        )
    }

    pub fn as_command_definition_with_metadata(
        &self,
        execution_target: CommandExecutionTargetDto,
        required_context: Vec<String>,
        required_capabilities: Vec<String>,
        keybind_capable: bool,
        dangerous: bool,
    ) -> CommandDefinitionDto {
        let sensitivity = CommandSensitivityDto::from_dangerous_flag(dangerous);
        CommandDefinitionDto {
            id: self.command_key.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            category: "User".to_string(),
            source_kind: CommandSourceKindDto::User,
            execution_target,
            arguments: vec![],
            required_context,
            required_capabilities,
            visible_in_palette: true,
            visible_in_context_menus: false,
            keybind_capable,
            dangerous,
            user_command_id: Some(self.id),
            // User-defined commands are human-only at Phase 1. Phase 5
            // surfaces Skill Macros as a separate AI path.
            nl_description: None,
            intent_samples: vec![],
            ai_callable: false,
            sensitivity,
            typed_verb: TypedVerb::Other,
            rate_limit_per_minute: None,
            feature: None,
        }
    }
}
