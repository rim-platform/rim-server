//! Command initiator classification — *who* asked the system to run a command.
//!
//! Phase 1 deliverable (H4 part 1) of the AI Agent Reframe per
//! `docs/design/ai-agent-reframe-design-2026-04-22-v2.md` §3.11.
//!
//! Every `ExecuteCommandCmd` / `ExecuteMacroCmd` is now tagged with the entry
//! point that produced it. The tag is consumed by:
//!
//! - **Audit log** (Phase 4 / Phase 5) — every executed command records
//!   `(actor, initiator, command_id, args_hash)`. Without `initiator` an
//!   audit row cannot distinguish a human-in-palette `chat.delete_message`
//!   from an agent-driven one — making Phase 5's incident-review surface
//!   useless for the failure modes Widow named.
//! - **Confirmation policy** (Phase 4) — `Agent`-initiated execution of a
//!   `Mutating` or `Restricted` command always requires a `ConfirmationProof`
//!   regardless of the per-command `dangerous` flag, while `Palette`-initiated
//!   execution of the same command may inline-confirm. The dispatch policy
//!   reads `(initiator, sensitivity, dangerous)` together.
//! - **Per-turn rate limiting** (Phase 4 / §3.7) — the agent's mutating-call
//!   cap is keyed off `initiator == Agent`. Other initiators are not capped.
//! - **CSP / IPC origin enforcement** (Phase 4 / C2) — the Tauri IPC proxy
//!   stamps `Agent` on completions originating from the agent webview and
//!   refuses any client-supplied initiator that doesn't match the origin.
//!
//! ## Wire contract (frontend codegen)
//!
//! Exposed as a GraphQL enum `CommandInitiator` with SCREAMING_SNAKE wire
//! values (`PALETTE`, `AGENT`, `MACRO`, `KEYBIND`, `CONTEXT_MENU`, `PLUGIN`,
//! `CLI`). The GraphQL `ExecuteCommandInput.initiator` field is
//! **optional** with backend default `Palette` so existing clients (and the
//! current frontend dispatcher, which has not been retrofitted) keep working
//! without a coordinated breaking change.
//!
//! Spider-Man's Phase 4 work will start sending the explicit initiator from
//! every dispatch site in `rim-ui`. Until then, the default-`Palette` rule
//! preserves Phase-1 conservative behaviour: anything that doesn't say it's
//! the agent is treated as a human action.
//!
//! ## Why an enum and not a free-form string
//!
//! A closed set keeps audit grep simple and lets the dispatch policy match
//! exhaustively (compile error if a new initiator is added without updating
//! every policy site). Plugin-defined initiators are intentionally NOT
//! supported — every plugin-issued command lands as `CommandInitiator::Plugin`,
//! and the plugin id travels in the audit row's actor context, not in the
//! initiator tag.

use std::fmt;

/// The entry point that produced an `ExecuteCommandCmd` or `ExecuteMacroCmd`.
///
/// Closed set — extending requires updating every match-site that consumes
/// initiator (dispatch policy, audit serializer, GraphQL `From` impl).
///
/// Per Fury v2 §3.11. Kept in `rim_services` (not `rim_core`) because the
/// transport-layer GraphQL enum (`rim_api::types::CommandInitiator`) mirrors
/// it via `From` impl — `rim_core` stays transport-neutral.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandInitiator {
    /// User typed/clicked in the command palette. Default for any callsite
    /// that has not been audited and explicitly tagged.
    Palette,
    /// AI agent issued the command via the dispatcher. Triggers per-turn
    /// mutating-call cap + always-confirm-for-Mutating policy.
    Agent,
    /// Inside a macro / user command expansion. The outermost macro's
    /// initiator is preserved on every nested `execute_command` call so a
    /// macro triggered by the agent stays tagged `Agent`, not `Macro`.
    Macro,
    /// Keyboard shortcut bound to a command.
    Keybind,
    /// Context menu (right-click) on a chat element.
    ContextMenu,
    /// Plugin's own runtime invoked the command (e.g. plugin-emitted action
    /// in response to an event the plugin subscribed to).
    Plugin,
    /// Out-of-process CLI client (rim-cli) invoked over the GraphQL API.
    Cli,
}

impl CommandInitiator {
    /// Returns `true` when this initiator is non-human and therefore subject
    /// to the agent-style mutating-call cap and always-confirm-for-Mutating
    /// rules. `Plugin` counts as agentic for safety; only `Palette`,
    /// `Keybind`, `ContextMenu`, `Cli`, and (transitively) `Macro` are human.
    ///
    /// `Macro` is treated as human here because a macro is a recorded series
    /// of human-issued steps; if an agent triggered the macro, the dispatch
    /// policy preserves the agent's initiator on the nested calls instead of
    /// reclassifying as `Macro`.
    pub fn is_agentic(self) -> bool {
        matches!(self, Self::Agent | Self::Plugin)
    }

    /// Lowercase ASCII identifier suitable for inclusion in audit-log
    /// `tracing` fields. Stable wire identifier — do not rename without
    /// coordinating with audit-log consumers.
    pub fn as_audit_str(self) -> &'static str {
        match self {
            Self::Palette => "palette",
            Self::Agent => "agent",
            Self::Macro => "macro",
            Self::Keybind => "keybind",
            Self::ContextMenu => "context_menu",
            Self::Plugin => "plugin",
            Self::Cli => "cli",
        }
    }
}

impl fmt::Display for CommandInitiator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_audit_str())
    }
}

impl Default for CommandInitiator {
    /// Default to `Palette` — back-compat for any caller that constructs
    /// `ExecuteCommandCmd` without setting initiator explicitly. This is the
    /// **conservative** default because `Palette` is human-attributed and
    /// does not trigger the agent-only rate limits / confirmation rules.
    /// A genuine missing-initiator bug surfaces as "human action attributed
    /// to a non-human caller" rather than the more dangerous reverse.
    fn default() -> Self {
        Self::Palette
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_and_plugin_are_agentic() {
        assert!(CommandInitiator::Agent.is_agentic());
        assert!(CommandInitiator::Plugin.is_agentic());
    }

    #[test]
    fn human_initiators_are_not_agentic() {
        assert!(!CommandInitiator::Palette.is_agentic());
        assert!(!CommandInitiator::Keybind.is_agentic());
        assert!(!CommandInitiator::ContextMenu.is_agentic());
        assert!(!CommandInitiator::Cli.is_agentic());
        assert!(!CommandInitiator::Macro.is_agentic());
    }

    #[test]
    fn default_is_palette() {
        // The conservative default. If this ever changes, every call site
        // that uses `..Default::default()` for ExecuteCommandCmd needs a
        // security re-review — flipping the default would silently re-tag
        // human actions as agentic (or vice versa).
        assert_eq!(CommandInitiator::default(), CommandInitiator::Palette);
    }

    #[test]
    fn audit_str_is_stable_lowercase_snake() {
        // Wire stability — these strings end up in the audit log and any
        // downstream log-shipping pipeline. Renaming requires coordinating
        // with the audit-log schema.
        assert_eq!(CommandInitiator::Palette.as_audit_str(), "palette");
        assert_eq!(CommandInitiator::Agent.as_audit_str(), "agent");
        assert_eq!(CommandInitiator::Macro.as_audit_str(), "macro");
        assert_eq!(CommandInitiator::Keybind.as_audit_str(), "keybind");
        assert_eq!(CommandInitiator::ContextMenu.as_audit_str(), "context_menu");
        assert_eq!(CommandInitiator::Plugin.as_audit_str(), "plugin");
        assert_eq!(CommandInitiator::Cli.as_audit_str(), "cli");
    }

    #[test]
    fn display_matches_audit_str() {
        // The Display impl is the same surface as as_audit_str — they must
        // agree so log-formatter callers don't get a different value than
        // structured-field callers.
        for v in [
            CommandInitiator::Palette,
            CommandInitiator::Agent,
            CommandInitiator::Macro,
            CommandInitiator::Keybind,
            CommandInitiator::ContextMenu,
            CommandInitiator::Plugin,
            CommandInitiator::Cli,
        ] {
            assert_eq!(format!("{}", v), v.as_audit_str());
        }
    }

    #[test]
    fn initiator_is_copy() {
        // Copy is intentional — initiator is a tiny tag passed through every
        // recursive command call (macros expand to N nested `execute_command`
        // invocations); requiring `.clone()` would be noise. If a future
        // variant gets a payload, this test breaks and forces a re-review.
        let a = CommandInitiator::Agent;
        let b = a; // Copy
        assert_eq!(a, b);
    }
}
