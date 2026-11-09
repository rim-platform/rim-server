//! Notification builtins. They only change the actor's own notification state, so
//! they need no capability (like `chat.mark_as_read`); `NotificationService` applies the
//! workspace-membership and conversation-access gates.

use rim_services::command::dto::{
    CommandArgumentDto, CommandArgumentTypeDto, CommandDefinitionDto, CommandExecutionTargetDto,
    CommandSourceKindDto,
};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;

use super::category;

/// The `duration` values of `notifications.mute_conversation`.
const MUTE_DURATIONS: [&str; 5] = ["1h", "8h", "until_tomorrow", "forever", "until"];

fn arg(
    name: &str,
    title: &str,
    description: &str,
    argument_type: CommandArgumentTypeDto,
    required: bool,
) -> CommandArgumentDto {
    CommandArgumentDto {
        name: name.to_string(),
        title: title.to_string(),
        description: description.to_string(),
        argument_type,
        required,
        default_value: None,
        allowed_values: None,
    }
}

fn enum_arg(name: &str, title: &str, description: &str, values: &[&str]) -> CommandArgumentDto {
    CommandArgumentDto {
        allowed_values: Some(values.iter().map(|v| v.to_string()).collect()),
        ..arg(name, title, description, CommandArgumentTypeDto::Enum, true)
    }
}

fn workspace_arg() -> CommandArgumentDto {
    arg(
        "server_id",
        "Workspace",
        "Workspace/server identifier.",
        CommandArgumentTypeDto::WorkspaceId,
        true,
    )
}

/// A backend, Mutating, agent-callable notification builtin.
fn notification_command(
    id: &str,
    title: &str,
    description: &str,
    arguments: Vec<CommandArgumentDto>,
    typed_verb: TypedVerb,
    nl_description: &str,
    intent_samples: &[&str],
) -> CommandDefinitionDto {
    CommandDefinitionDto {
        id: id.to_string(),
        title: title.to_string(),
        description: description.to_string(),
        category: category::NOTIFICATIONS.to_string(),
        source_kind: CommandSourceKindDto::BuiltIn,
        execution_target: CommandExecutionTargetDto::Backend,
        arguments,
        required_context: vec![],
        required_capabilities: vec![],
        visible_in_palette: true,
        visible_in_context_menus: false,
        keybind_capable: true,
        dangerous: false,
        user_command_id: None,
        nl_description: Some(nl_description.to_string()),
        intent_samples: intent_samples.iter().map(|s| s.to_string()).collect(),
        ai_callable: true,
        sensitivity: CommandSensitivityDto::Mutating,
        typed_verb,
        rate_limit_per_minute: None,
    }
}

pub(super) fn commands() -> Vec<CommandDefinitionDto> {
    vec![
        notification_command(
            "notifications.set_level",
            "Set Notification Level",
            "Choose which messages in a conversation notify you: all, mentions, none, or the \
             workspace default.",
            vec![
                workspace_arg(),
                arg(
                    "conversation_id",
                    "Conversation",
                    "Conversation to configure; defaults to the focused conversation.",
                    CommandArgumentTypeDto::ConversationId,
                    true,
                ),
                enum_arg(
                    "level",
                    "Level",
                    "all (every message), mentions (mentions, direct messages, keywords and \
                     thread replies), none (nothing), or default (use the workspace default).",
                    &["all", "mentions", "none", "default"],
                ),
            ],
            TypedVerb::Configure,
            "Set the user's own notification level for a conversation: all notifies on every \
             message, mentions only on mentions, direct messages, keyword hits and replies in \
             their threads, none on nothing at all, and default goes back to the workspace \
             default. Only affects the user who runs it. Use when the user asks to be notified \
             about every message, only when mentioned, or about nothing in a conversation; a \
             mute for a while is `notifications.mute_conversation`.",
            &[
                "never notify me about this channel",
                "only notify me about mentions here",
                "notify me about every message in this channel",
                "use my default notification setting here",
            ],
        ),
        notification_command(
            "notifications.dnd",
            "Do Not Disturb",
            "Pause notification toasts and sounds (on, off, or until a time). Notifications are \
             still recorded in your inbox.",
            vec![
                workspace_arg(),
                enum_arg(
                    "mode",
                    "Mode",
                    "on (until turned off), off, or until (needs `until` or `minutes`).",
                    &["on", "off", "until"],
                ),
                arg(
                    "until",
                    "Until",
                    "RFC 3339 time when do-not-disturb ends (mode `until`).",
                    CommandArgumentTypeDto::String,
                    false,
                ),
                arg(
                    "minutes",
                    "Minutes",
                    "Minutes from now when do-not-disturb ends (mode `until`, instead of \
                     `until`; 1 to 10080).",
                    CommandArgumentTypeDto::Integer,
                    false,
                ),
            ],
            TypedVerb::Toggle,
            "Turn the user's do-not-disturb pause on, off, or on until a given time. While it \
             is on, notifications are still recorded in the inbox and digest but clients do not \
             show toasts or play sounds. It is separate from the daily do-not-disturb window in \
             notification settings. Use when the user asks not to be disturbed, to pause or \
             snooze notifications, or to resume them.",
            &[
                "do not disturb me",
                "pause notifications for two hours",
                "turn off do not disturb",
                "snooze notifications until 5pm",
            ],
        ),
        notification_command(
            "notifications.mute_conversation",
            "Mute Conversation…",
            "Mute a conversation for 1 hour, 8 hours, until tomorrow morning, until a time, or \
             until you unmute it.",
            vec![
                workspace_arg(),
                arg(
                    "conversation_id",
                    "Conversation",
                    "Conversation to mute; defaults to the focused conversation.",
                    CommandArgumentTypeDto::ConversationId,
                    true,
                ),
                enum_arg(
                    "duration",
                    "Duration",
                    "1h, 8h, until_tomorrow (09:00 tomorrow), forever (until unmuted), or until \
                     (needs `until`).",
                    &MUTE_DURATIONS,
                ),
                arg(
                    "until",
                    "Until",
                    "RFC 3339 time when the mute ends (duration `until`).",
                    CommandArgumentTypeDto::String,
                    false,
                ),
            ],
            TypedVerb::Configure,
            "Mute a conversation for the user only: it stops notifying them until the mute ends. \
             1h and 8h count from now, until_tomorrow ends at 09:00 tomorrow, forever lasts \
             until they unmute it, and until ends at a given RFC 3339 time. Use when the user \
             asks to mute, silence or snooze a conversation for a while.",
            &[
                "mute this channel for an hour",
                "silence this conversation until tomorrow",
                "mute #random for good",
            ],
        ),
        notification_command(
            "notifications.unmute_conversation",
            "Unmute Conversation",
            "End a conversation's mute.",
            vec![
                workspace_arg(),
                arg(
                    "conversation_id",
                    "Conversation",
                    "Conversation to unmute; defaults to the focused conversation.",
                    CommandArgumentTypeDto::ConversationId,
                    true,
                ),
            ],
            TypedVerb::Configure,
            "Unmute a conversation the user muted, timed or not, so it notifies them again at \
             their level for it. Only affects the user who runs it. Use when the user asks to \
             unmute a conversation.",
            &[
                "unmute this channel",
                "turn notifications for this conversation back on",
            ],
        ),
        notification_command(
            "notifications.mark_all_read",
            "Mark All Notifications Read",
            "Mark every notification in this workspace as read.",
            vec![workspace_arg()],
            TypedVerb::Write,
            "Mark all of the user's notifications in the workspace as read, which clears the \
             notification badge. Messages and conversation unread counts are not changed. Use \
             when the user asks to clear or dismiss their notifications.",
            &[
                "mark all notifications as read",
                "clear my notifications",
                "dismiss all notifications",
            ],
        ),
    ]
}
