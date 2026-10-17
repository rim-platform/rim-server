//! Command initiator classification: *who* asked the system to run a command.
//!
//! Every `ExecuteCommandCmd` / `ExecuteMacroCmd` carries the entry point that produced it. The
//! tag is consumed by:
//!
//! - **Audit log**: every executed command records its initiator next to the actor, command id
//!   and argument hash, so a palette `chat.delete_message` is distinguishable from an
//!   agent-driven one.
//! - **Confirmation policy**: an agentic initiator running a `Mutating` command needs a
//!   `ConfirmationProof` even when the command is not `dangerous`; `dangerous` and `Restricted`
//!   commands need one from every initiator. The dispatch policy reads
//!   `(initiator, sensitivity, dangerous)` together.
//! - **AI command surface switch**: an `Agent` call fails while the instance has the AI command
//!   surface turned off.
//!
//! The initiator is asserted by the client; the server does not verify where a call came from.
//!
//! ## Wire contract
//!
//! Exposed as the GraphQL enum `CommandInitiator` (`PALETTE`, `AGENT`, `MACRO`, `KEYBIND`,
//! `CONTEXT_MENU`, `PLUGIN`, `CLI`). `ExecuteCommandInput.initiator` is optional and defaults to
//! `Palette`: anything that does not say it is the agent is treated as a human action. The
//! first-party client sends `AGENT` from its agent tools.
//!
//! ## Why an enum and not a free-form string
//!
//! A closed set keeps audit grep simple and lets the dispatch policy match exhaustively (adding
//! an initiator is a compile error until every policy site handles it). A command a plugin's
//! runtime issues on its own is tagged `CommandInitiator::Plugin`.

use std::fmt;

/// The entry point that produced an `ExecuteCommandCmd` or `ExecuteMacroCmd`.
///
/// Closed set — extending requires updating every match-site that consumes
/// initiator (dispatch policy, audit serializer, GraphQL `From` impl).
///
/// Kept in `rim_services` (not `rim_core`) because the transport-layer GraphQL enum
/// (`rim_api::types::CommandInitiator`) mirrors it via `From` impl — `rim_core` stays
/// transport-neutral.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandInitiator {
    /// User typed/clicked in the command palette. The default when a caller
    /// does not set an initiator.
    Palette,
    /// AI agent issued the command via the dispatcher. Triggers the
    /// always-confirm-for-Mutating policy.
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
    /// to the always-confirm-for-Mutating rule. `Plugin` counts as agentic for
    /// safety; only `Palette`, `Keybind`, `ContextMenu`, `Cli`, and
    /// (transitively) `Macro` are human.
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
    /// Returns `Palette`, the initiator of an `ExecuteCommandCmd` built
    /// without one. This is the **conservative** default because `Palette` is
    /// human-attributed and does not trigger the agent-only confirmation
    /// rules. A genuine missing-initiator bug surfaces as "human action
    /// attributed to a non-human caller" rather than the more dangerous
    /// reverse.
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
        // The conservative default. Every call site that uses
        // `..Default::default()` for ExecuteCommandCmd relies on it: flipping
        // the default would silently re-tag human actions as agentic (or vice
        // versa).
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
        // Initiator is a tiny tag passed through every recursive command call
        // (macros expand to N nested `execute_command` invocations), so it is
        // Copy. A variant with a non-Copy payload fails this test.
        let a = CommandInitiator::Agent;
        let b = a; // Copy
        assert_eq!(a, b);
    }
}
