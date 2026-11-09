//! Builtins that change only the caller's own state, which the timeout gate lets through and
//! some of which write no command audit row.

/// Builtins that change only the caller's own view of a workspace: their read state. Together
/// with every command under [`NOTIFICATION_COMMAND_PREFIX`] they write no command audit row,
/// since the workspace's Owners and Moderators read that log and rows of them would show a
/// person's reading habits.
const OWN_VIEW_COMMANDS: &[&str] = &["chat.mark_as_read", "chat.mark_as_unread"];

/// Builtins that change only the caller's own state beyond their view: leaving a conversation,
/// which the other members see, so it keeps its audit row.
const OWN_MEMBERSHIP_COMMANDS: &[&str] = &["chat.leave_conversation"];

/// The prefix of the notification builtins, which change only the caller's own notification
/// settings.
const NOTIFICATION_COMMAND_PREFIX: &str = "notifications.";

/// Whether `command_id` changes only the caller's own view (read state and notification
/// settings), so its run writes no command audit row.
pub(crate) fn changes_only_callers_view(command_id: &str) -> bool {
    OWN_VIEW_COMMANDS.contains(&command_id) || command_id.starts_with(NOTIFICATION_COMMAND_PREFIX)
}

/// Whether `command_id` changes only the caller's own state, so a timed-out caller may run it.
pub(crate) fn changes_only_callers_state(command_id: &str) -> bool {
    changes_only_callers_view(command_id) || OWN_MEMBERSHIP_COMMANDS.contains(&command_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_state_and_notification_commands_change_only_the_view() {
        for id in [
            "chat.mark_as_read",
            "chat.mark_as_unread",
            "notifications.set_level",
            "notifications.dnd",
            "notifications.mark_all_read",
            "notifications.mute_conversation",
            "notifications.unmute_conversation",
        ] {
            assert!(changes_only_callers_view(id), "{id}");
            assert!(changes_only_callers_state(id), "{id}");
        }
    }

    #[test]
    fn leaving_changes_own_state_but_not_only_the_view() {
        assert!(changes_only_callers_state("chat.leave_conversation"));
        assert!(!changes_only_callers_view("chat.leave_conversation"));
    }

    #[test]
    fn commands_others_see_are_not_own_state() {
        for id in [
            "chat.pin_message",
            "chat.post_message",
            "chat.add_reaction",
            "admin.kick",
            "state.set",
            "user.notifications.mute",
            "notifications",
        ] {
            assert!(!changes_only_callers_state(id), "{id}");
            assert!(!changes_only_callers_view(id), "{id}");
        }
    }
}
