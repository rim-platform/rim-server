//! App commands: built-in commands the client runs itself. The server never lists, runs or
//! offers them to an agent; it knows their ids so that user and workspace keybinds can name them.

use rim_services::command::dto::{
    CommandDefinitionDto, CommandExecutionTargetDto, CommandSourceKindDto,
};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;

use super::category;

/// One app command: its id, the title the client shows, its category and whether a keybind may
/// name it.
pub(crate) struct AppCommand {
    /// The command id.
    pub(crate) id: &'static str,
    /// The title the client shows.
    pub(crate) title: &'static str,
    /// One of [`category::ALL`].
    pub(crate) category: &'static str,
    /// Whether a keybind may name it.
    pub(crate) keybind_capable: bool,
}

const fn app(id: &'static str, title: &'static str, category: &'static str) -> AppCommand {
    AppCommand {
        id,
        title,
        category,
        keybind_capable: true,
    }
}

/// Every app command the server knows.
pub(crate) const APP_COMMANDS: &[AppCommand] = &[
    app(
        "workspace.show_shortcuts",
        "Show Keyboard Shortcuts",
        category::VIEW,
    ),
    app(
        "workspace.toggle_vim_mode",
        "Toggle Vim Mode",
        category::VIEW,
    ),
    app(
        "workspace.previous_unread_channel",
        "Previous Unread Conversation",
        category::NAVIGATION,
    ),
    app(
        "workspace.navigate_panel_left",
        "Focus the Pane to the Left",
        category::NAVIGATION,
    ),
    app(
        "workspace.navigate_panel_right",
        "Focus the Pane to the Right",
        category::NAVIGATION,
    ),
    app(
        "workspace.cycle_layout_preset",
        "Cycle Layout Preset",
        category::VIEW,
    ),
    app(
        "workspace.toggle_zen_mode",
        "Toggle Zen Mode",
        category::VIEW,
    ),
    app(
        "workspace.back_to_workspace",
        "Back to Workspace",
        category::NAVIGATION,
    ),
    app(
        "workspace.cycle_display_density",
        "Cycle Display Density",
        category::VIEW,
    ),
    app("workspace.rename", "Rename Workspace…", category::WORKSPACE),
    app(
        "workspace.open_bots",
        "Open Bots",
        category::BOTS_AND_WEBHOOKS,
    ),
    app("workspace.leave", "Leave Workspace…", category::WORKSPACE),
    app("workspace.open_agents", "Open Agents", category::AGENTS),
    app("workspace.open_unreads", "Open Unreads", category::UNREADS),
    app(
        "workspace.unreads_next",
        "Unreads: Next Message",
        category::UNREADS,
    ),
    app(
        "workspace.unreads_previous",
        "Unreads: Previous Message",
        category::UNREADS,
    ),
    app(
        "workspace.unreads_mark_group",
        "Unreads: Mark Conversation as Read",
        category::UNREADS,
    ),
    app(
        "workspace.unreads_open",
        "Unreads: Open Message",
        category::UNREADS,
    ),
    app("workspace.unreads_undo", "Unreads: Undo", category::UNREADS),
    app("workspace.open_home", "Open Home", category::HOME),
    AppCommand {
        keybind_capable: false,
        ..app("workspace.edit_home", "Edit Home", category::HOME)
    },
    app(
        "workspace.open_audit_log",
        "Open Audit Log",
        category::WORKSPACE,
    ),
    app(
        "workspace.open_message_requests",
        "Open Message Requests",
        category::PEOPLE,
    ),
    app(
        "workspace.toggle_pinned_messages",
        "Toggle Pinned Messages",
        category::CONVERSATIONS,
    ),
    app(
        "workspace.toggle_channel_bots",
        "Toggle Bots in This Channel",
        category::BOTS_AND_WEBHOOKS,
    ),
    app(
        "workspace.toggle_notifications",
        "Toggle Notifications",
        category::NOTIFICATIONS,
    ),
    app(
        "workspace.toggle_conversation_members",
        "Toggle Conversation Members",
        category::CONVERSATIONS,
    ),
    app(
        "workspace.toggle_member_directory",
        "Toggle Workspace Members",
        category::WORKSPACE,
    ),
    app(
        "workspace.toggle_lens_picker",
        "Toggle Channel Lenses",
        category::CONVERSATIONS,
    ),
    app("workspace.go_back", "Go Back", category::NAVIGATION),
    app("workspace.go_forward", "Go Forward", category::NAVIGATION),
    app("app.quit", "Quit RIM", category::APP),
];

impl AppCommand {
    /// The definition keybind validation checks this app command with: client-run, `Safe`,
    /// never AI-callable and in no list the server serves.
    pub(crate) fn definition(&self) -> CommandDefinitionDto {
        CommandDefinitionDto {
            id: self.id.to_string(),
            title: self.title.to_string(),
            description: String::new(),
            category: self.category.to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::DesktopLocal,
            arguments: vec![],
            required_context: vec![],
            required_capabilities: vec![],
            visible_in_palette: false,
            visible_in_context_menus: false,
            keybind_capable: self.keybind_capable,
            dangerous: false,
            user_command_id: None,
            nl_description: None,
            intent_samples: vec![],
            ai_callable: false,
            sensitivity: CommandSensitivityDto::Safe,
            typed_verb: TypedVerb::Other,
            rate_limit_per_minute: None,
        }
    }
}

/// The definition of the app command `command_id`, or `None` when it names none.
pub(crate) fn app_command(command_id: &str) -> Option<CommandDefinitionDto> {
    APP_COMMANDS
        .iter()
        .find(|command| command.id == command_id)
        .map(AppCommand::definition)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::registry::{builtin_commands, collect_ai_callable_inventory};

    /// The reserved ids of the app commands that approve or refuse an outside agent's request
    /// or pairing.
    const AGENT_APPROVAL_IDS: [&str; 4] = [
        "agents.approve_request",
        "agents.refuse_request",
        "agents.approve_pairing",
        "agents.refuse_pairing",
    ];

    #[test]
    fn app_commands_and_builtins_share_no_id() {
        let builtins = builtin_commands();
        for command in APP_COMMANDS {
            assert!(
                builtins.iter().all(|builtin| builtin.id != command.id),
                "{} is both an app command and a builtin",
                command.id
            );
        }
    }

    #[test]
    fn app_command_ids_are_unique_and_categorised() {
        let mut ids: Vec<&str> = APP_COMMANDS.iter().map(|command| command.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate app command id");
        for command in APP_COMMANDS {
            assert!(
                category::ALL.contains(&command.category),
                "{}: {}",
                command.id,
                command.category
            );
        }
    }

    #[test]
    fn app_command_definitions_are_never_ai_callable() {
        for command in APP_COMMANDS {
            let definition = command.definition();
            assert!(!definition.ai_callable, "{}", command.id);
            assert_eq!(definition.sensitivity, CommandSensitivityDto::Safe);
        }
        assert!(app_command("workspace.open_home").is_some_and(|c| c.keybind_capable));
        assert!(app_command("workspace.edit_home").is_some_and(|c| !c.keybind_capable));
        assert!(app_command("chat.post_message").is_none());
    }

    #[test]
    fn agent_approval_commands_are_offered_to_no_agent() {
        let builtins = builtin_commands();
        let ai_callable = collect_ai_callable_inventory(&builtins);
        for id in AGENT_APPROVAL_IDS {
            assert!(!ai_callable.contains(&id), "{id}");
            assert!(builtins.iter().all(|builtin| builtin.id != id), "{id}");
            assert!(
                app_command(id).is_none_or(|definition| !definition.ai_callable),
                "{id}"
            );
        }
    }
}
