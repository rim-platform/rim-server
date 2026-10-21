mod admin;
mod chat;
mod chat_messages;
mod conversation;
mod member;
mod moderation;
mod notification;
mod pin;
mod reaction;
mod state;
mod workspace;

use rim_core::server_member::model::ServerMemberRole;
use rim_services::command::dto::CommandDefinitionDto;
use rim_services::command::sanitize::sanitize_command_ai_fields;

pub(crate) fn builtin_commands() -> Vec<CommandDefinitionDto> {
    let mut defs = builtin_commands_raw();
    // Sanitize AI-facing metadata (`nl_description`, `intent_samples`) at
    // registration time.
    //
    // `expect` is appropriate here: built-ins are developer-authored and
    // any sanitize failure is a CI-caught bug, not a runtime input error.
    for def in &mut defs {
        sanitize_command_ai_fields(def)
            .expect("built-in command AI metadata must pass sanitization (developer error)");
    }
    defs
}

/// Returns the IDs of every command flagged `ai_callable: true`,
/// in registration order. Pure helper so the audit emission can be tested
/// without capturing a tracing subscriber.
pub(super) fn collect_ai_callable_inventory(commands: &[CommandDefinitionDto]) -> Vec<&str> {
    commands
        .iter()
        .filter(|c| c.ai_callable)
        .map(|c| c.id.as_str())
        .collect()
}

/// Startup audit emission. Walks the current builtin
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
        "AI-callable command inventory emitted at startup"
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
        moderation::commands(),
        notification::commands(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The lowest workspace role a builtin needs, matching the role gate of the service behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequiredRole {
    /// Any member, guests included.
    Guest,
    /// A member, moderator or owner; in the direct space, where everyone is a guest, a guest too.
    MemberOrDirectSpaceGuest,
    /// A member, moderator or owner.
    Member,
    /// A moderator or the owner.
    Moderator,
    /// The owner.
    Owner,
}

impl RequiredRole {
    /// Whether a member holding `role` meets the requirement; `direct_space` when the workspace
    /// is the direct space.
    pub(crate) fn admits(self, role: ServerMemberRole, direct_space: bool) -> bool {
        match self {
            Self::Guest => true,
            Self::MemberOrDirectSpaceGuest => !role.is_guest() || direct_space,
            Self::Member => !role.is_guest(),
            Self::Moderator => role.is_moderator(),
            Self::Owner => role == ServerMemberRole::Owner,
        }
    }
}

/// The role `command_id` needs in its workspace. `Guest` for every id not listed, including
/// commands that are not builtins.
pub(crate) fn required_role(command_id: &str) -> RequiredRole {
    match command_id {
        "admin.invite_member" => RequiredRole::Owner,
        "admin.kick"
        | "admin.ban"
        | "admin.unban"
        | "admin.timeout"
        | "admin.set_role"
        | "chat.set_slow_mode"
        | "chat.archive_channel"
        | "chat.unarchive_channel" => RequiredRole::Moderator,
        "chat.create_channel" | "chat.create_group" | "chat.invite_to_conversation" => {
            RequiredRole::Member
        }
        "chat.join_channel" => RequiredRole::MemberOrDirectSpaceGuest,
        _ => RequiredRole::Guest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_role_admits_by_role_and_space() {
        use ServerMemberRole::{Guest, Member, Moderator, Owner};
        // Lowest admitted role in an ordinary workspace, then in the direct space.
        let cases = [
            (RequiredRole::Guest, Guest, Guest),
            (RequiredRole::MemberOrDirectSpaceGuest, Member, Guest),
            (RequiredRole::Member, Member, Member),
            (RequiredRole::Moderator, Moderator, Moderator),
            (RequiredRole::Owner, Owner, Owner),
        ];
        let ranked = [Guest, Member, Moderator, Owner];
        let rank = |role| ranked.iter().position(|r| *r == role).unwrap_or(0);
        for (required, lowest, lowest_in_direct) in cases {
            for role in ranked {
                assert_eq!(
                    required.admits(role, false),
                    rank(role) >= rank(lowest),
                    "{required:?} {role}"
                );
                assert_eq!(
                    required.admits(role, true),
                    rank(role) >= rank(lowest_in_direct),
                    "{required:?} {role} in the direct space"
                );
            }
        }
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
    /// 2. A refactor that moves sanitization into `builtin_commands_raw()`
    ///    by mistake is caught here rather than relying on
    ///    `builtin_commands_ids_are_unique` to incidentally exercise the
    ///    wrapper's panic path.
    ///
    /// Kept as its own test so a failure names the sanitization step.
    #[test]
    fn builtin_commands_sanitize_cleanly() {
        let _ = builtin_commands();
    }

    // ── AI-callable inventory ──

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
        // The builtins the agent may call. Every one is authorized by the service behind it, so
        // the agent gains no access the user does not already have. Membership, moderation and
        // notification commands are gated by `ConversationService`, `ModerationService` and
        // `NotificationService`; the desktop-local navigation commands run on the client; every
        // agent write is Mutating and therefore confirmation-gated. Left out on purpose:
        // `chat.remove_conversation_member`, `admin.kick`, `admin.ban`, `admin.unban` and
        // `admin.set_role` (dangerous); the commands that act on the user's selected message
        // (`chat.edit_message`, `chat.delete_message`, `chat.reply_to_selected_message`,
        // `chat.copy_message_text`), since the agent cannot supply a selection; and
        // `admin.invite_member`, whose payload is an invite token that would pass through the
        // model provider.
        //
        // Set-equality assertion: the security contract is that exactly these IDs are
        // AI-callable, so adding an AI-callable builtin without a security review (and updating
        // this set) fails CI. Order is not asserted: registration order is a domain-file
        // concern, not a security property.
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
