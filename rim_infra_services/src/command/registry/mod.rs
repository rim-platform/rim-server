mod admin;
mod chat;
mod chat_messages;
mod conversation;
mod incident;
mod member;
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
/// `target = "audit"` tracing line so operators can confirm at boot which
/// commands the LLM surface can reach.
///
/// Called from `rim_server::app::build_app_runtime` after the capability
/// registry is seeded but before the HTTP listener binds.
pub fn emit_ai_callable_inventory_audit() {
    let commands = builtin_commands();
    let ai_callable_ids = collect_ai_callable_inventory(&commands);
    tracing::info!(
        target = "audit",
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
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn collect_ai_callable_inventory_phase4_a5_yields_twelve_commands() {
        // P3 + P4-A4 yielded 7 ai_callable builtins. P4-A5 adds 5 brand-new
        // commands (`chat.list_members`, `chat.search_messages`,
        // `chat.get_recent_messages`, `chat.pin_message`,
        // `chat.toggle_reaction`) for a total of 12.
        //
        // Set-equality assertion: the security contract is that exactly these
        // 12 IDs are AI-callable. Adding a new ai_callable builtin without a
        // Widow security review (and updating this set) must fail CI.
        // Order is intentionally NOT asserted — registration order is a
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
