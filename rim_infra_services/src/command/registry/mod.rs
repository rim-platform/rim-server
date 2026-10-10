mod admin;
mod chat;
mod chat_messages;
mod conversation;
mod incident;
mod member;
mod moderation;
mod notification;
mod oncall;
mod pin;
mod reaction;
mod state;
mod workspace;

use rim_services::command::dto::CommandDefinitionDto;
use rim_services::command::sanitize::sanitize_command_ai_fields;

pub(crate) fn builtin_commands() -> Vec<CommandDefinitionDto> {
    let mut defs = builtin_commands_raw();
    // Sanitize AI-facing metadata at registration time. Phase 1 builtins
    // never populate `nl_description` or `intent_samples`, so this is a
    // defensive no-op. Wired now so Phase 2 generator + Phase 5 plugin
    // install land into a path that's already proven by Hawkeye Gate 2.
    //
    // `expect` is appropriate here: built-ins are developer-authored and
    // any sanitize failure is a CI-caught bug, not a runtime input error.
    // Plugin/user paths handle SanitizeError as a regular Result.
    for def in &mut defs {
        sanitize_command_ai_fields(def)
            .expect("built-in command AI metadata must pass sanitization (developer error)");
    }
    defs
}

/// **F1 closure.** Returns the IDs of every command flagged `ai_callable: true`,
/// in registration order. Pure helper so the audit emission can be tested
/// without capturing a tracing subscriber.
pub(super) fn collect_ai_callable_inventory(commands: &[CommandDefinitionDto]) -> Vec<&str> {
    commands
        .iter()
        .filter(|c| c.ai_callable)
        .map(|c| c.id.as_str())
        .collect()
}

/// **F1 closure — startup audit emission.** Walks the current builtin
/// command registry, computes the AI-callable inventory, and emits a
/// `target: "audit"` tracing line so operators can confirm at boot which
/// commands the LLM surface can reach.
///
/// Called from `rim_server::app::build_app_runtime` after the capability
/// registry is seeded but before the HTTP listener binds.
pub fn emit_ai_callable_inventory_audit() {
    let commands = builtin_commands();
    let ai_callable_ids = collect_ai_callable_inventory(&commands);
    tracing::info!(
        target: "audit",
        event = "ai_callable_inventory",
        count = ai_callable_ids.len(),
        commands = ?ai_callable_ids,
        "AI-callable command inventory emitted at startup (Phase 1 §3.4 / F1)"
    );
}

fn builtin_commands_raw() -> Vec<CommandDefinitionDto> {
    [
        chat_messages::commands(),
        conversation::commands(),
        workspace::commands(),
        chat::commands(),
        member::commands(),
        pin::commands(),
        reaction::commands(),
        state::commands(),
        admin::commands(),
        incident::commands(),
        oncall::commands(),
        moderation::commands(),
        notification::commands(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Track 0.4: exactly the four incident/on-call builtins belong to the `incidents`
    /// workspace feature; every other builtin is always available.
    #[test]
    fn only_incident_and_oncall_builtins_are_feature_gated() {
        let commands = builtin_commands();
        let mut gated: Vec<&str> = commands
            .iter()
            .filter(|c| {
                c.feature == Some(rim_core::workspace_feature::model::WorkspaceFeature::Incidents)
            })
            .map(|c| c.id.as_str())
            .collect();
        gated.sort_unstable();
        assert_eq!(
            gated,
            [
                "incident.ack",
                "incident.escalate",
                "oncall.take_shift",
                "oncall.who"
            ]
        );
    }

    #[test]
    fn builtin_commands_ids_are_unique() {
        let commands = builtin_commands();
        let mut ids: Vec<&str> = commands.iter().map(|c| c.id.as_str()).collect();
        let original_len = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), original_len, "duplicate command IDs found");
    }

    /// Explicit CI gate that `builtin_commands()` (which sanitizes AI metadata
    /// via `sanitize_command_ai_fields`) does not panic on the current built-in
    /// definitions. The wrapper's `.expect("…")` exists because failure here is
    /// a developer-time bug (a builtin literal added with malformed
    /// `nl_description` or `intent_samples`), not a runtime input. This test
    /// names that contract so that:
    ///
    /// 1. A CI run on a feature branch fails *before* the change reaches main
    ///    instead of panicking at server bootstrap post-deploy, and
    /// 2. Future refactors that move sanitization into `builtin_commands_raw()`
    ///    by mistake are caught here rather than relying on
    ///    `builtin_commands_ids_are_unique` to incidentally exercise the
    ///    wrapper's panic path.
    ///
    /// Per Hawkeye Gate 2 BLOCK + WARN. Do not fold into another test.
    #[test]
    fn builtin_commands_sanitize_cleanly() {
        let _ = builtin_commands();
    }

    // ── F1 inventory ──

    #[test]
    fn collect_ai_callable_inventory_returns_only_ai_callable_ids() {
        let mut a = builtin_commands().into_iter().next().unwrap();
        a.id = "a".to_string();
        a.ai_callable = true;
        let mut b = builtin_commands().into_iter().next().unwrap();
        b.id = "b".to_string();
        b.ai_callable = false;
        let mut c = builtin_commands().into_iter().next().unwrap();
        c.id = "c".to_string();
        c.ai_callable = true;

        let cmds = [a, b, c];
        let ids = collect_ai_callable_inventory(&cmds);
        assert_eq!(ids, vec!["a", "c"]);
    }

    #[test]
    fn collect_ai_callable_inventory_matches_reviewed_set() {
        // P3 + P4-A4 yielded 7 ai_callable builtins. P4-A5 adds 5 brand-new
        // commands (`chat.list_members`, `chat.search_messages`,
        // `chat.get_recent_messages`, `chat.pin_message`,
        // `chat.toggle_reaction`) for a total of 12. Track C2
        // (`docs/specs/track-c2-agent-operator.md` §1, Jakub-ratified) flips
        // 4 more C1 lifecycle commands to `ai_callable: true` — `incident.ack`,
        // `incident.escalate`, `oncall.who`, `oncall.take_shift` — for a new
        // total of 16. Track 1.1 (`docs/specs/track-1-1-private-channels.md`, Commands) adds
        // five chat membership/archive builtins — `chat.join_channel`,
        // `chat.leave_conversation`, `chat.invite_to_conversation`, `chat.archive_channel`,
        // `chat.unarchive_channel` — for 21. `chat.remove_conversation_member` is deliberately
        // NOT AI-callable (dangerous, confirmation-gated). Every one of them is authorized by
        // `ConversationService` (membership, private-channel visibility, Owner/Moderator for
        // archive), so the agent gains no access the user does not already have. Track 1.2
        // (`docs/specs/track-1-2-roles-and-moderation.md`, Commands) adds three moderation
        // builtins — `chat.set_slow_mode`, `admin.timeout`, `chat.report_message` — for 24. All
        // three are authorized by `ModerationService` (Owner/Moderator for slow mode and
        // timeouts, read access for reports). `admin.kick`, `admin.ban`, `admin.unban` and
        // `admin.set_role` are deliberately NOT AI-callable. Track 1.3
        // (`docs/specs/track-1-3-notification-rules.md`, Commands) adds three notification
        // builtins — `notifications.set_level`, `notifications.dnd`,
        // `notifications.mark_all_read` — for 27. They change only the actor's own notification
        // state, gated by `NotificationService` (workspace membership; conversation access for
        // the level), so they need no capability. Track 4.1 (ROADMAP Track 4.1) flips the
        // general builtins for 45: eleven desktop-local navigation commands the client runs
        // locally (Safe, no confirmation), `state.get`/`state.list` (Safe reads), and
        // `chat.mark_as_read`, `chat.create_group`, `state.set`, `state.incr`, `state.delete`,
        // raised from Safe to Mutating so every agent write is confirmation-gated. Commands that
        // act on the user's selected message (`chat.edit_message`, `chat.delete_message`,
        // `chat.reply_to_selected_message`, `chat.copy_message_text`) stay off because the agent
        // cannot supply a selection, and `admin.invite_member` stays off because its payload is
        // an invite token that would pass through the model provider.
        //
        // Set-equality assertion: the security contract is that exactly these
        // 45 IDs are AI-callable. Adding a new ai_callable builtin without a
        // Hawkeye + Widow security review (and updating this set) must fail
        // CI. Order is intentionally NOT asserted — registration order is a
        // domain-file concern, not a security property. Use the audit log
        // snapshot in ops dashboards for order-sensitive monitoring.
        let cmds = builtin_commands();
        let mut ids: Vec<&str> = collect_ai_callable_inventory(&cmds);
        ids.sort();

        let mut expected = vec![
            "chat.post_message",
            "chat.open_thread",
            "workspace.open_settings",
            "workspace.navigate_to_channel",
            "workspace.switch_workspace",
            "chat.list_channels",
            "chat.create_channel",
            "chat.list_members",
            "chat.search_messages",
            "chat.get_recent_messages",
            "chat.pin_message",
            "chat.toggle_reaction",
            "incident.ack",
            "incident.escalate",
            "oncall.who",
            "oncall.take_shift",
            "chat.join_channel",
            "chat.leave_conversation",
            "chat.invite_to_conversation",
            "chat.archive_channel",
            "chat.unarchive_channel",
            "chat.set_slow_mode",
            "admin.timeout",
            "chat.report_message",
            "notifications.set_level",
            "notifications.dnd",
            "notifications.mark_all_read",
            "workspace.open_search",
            "workspace.search_in_channel",
            "workspace.open_marketplace",
            "workspace.toggle_sidebar",
            "workspace.previous_channel",
            "workspace.next_channel",
            "workspace.next_unread_channel",
            "workspace.close_panel",
            "workspace.reopen_last_thread",
            "chat.scroll_to_bottom",
            "chat.scroll_to_top",
            "chat.mark_as_read",
            "chat.create_group",
            "state.get",
            "state.list",
            "state.set",
            "state.incr",
            "state.delete",
        ];
        expected.sort();

        assert_eq!(ids, expected);
    }

    #[test]
    fn collect_ai_callable_inventory_preserves_registration_order() {
        let mut a = builtin_commands().into_iter().next().unwrap();
        a.id = "z.last".to_string();
        a.ai_callable = true;
        let mut b = builtin_commands().into_iter().next().unwrap();
        b.id = "a.first".to_string();
        b.ai_callable = true;
        // Inventory is consumed by ops dashboards downstream — preserving
        // input order keeps the audit line stable across deploys.
        let cmds = [a, b];
        let ids = collect_ai_callable_inventory(&cmds);
        assert_eq!(ids, vec!["z.last", "a.first"]);
    }
}
