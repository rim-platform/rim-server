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
    app(
        "workspace.open_agent_sessions",
        "Open Agent Sessions",
        category::AGENTS,
    ),
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
    app(
        "plugins.open_settings",
        "Open Plugin Settings",
        category::PLUGINS,
    ),
    app(
        "workspace.open_direct_messages",
        "Open Direct Messages",
        category::NAVIGATION,
    ),
    app("workspace.add", "Add a Workspace…", category::WORKSPACE),
    app(
        "workspace.open_workspace_settings",
        "Open Workspace Settings",
        category::WORKSPACE,
    ),
    app(
        "workspace.open_join_requests",
        "Open Join Requests",
        category::WORKSPACE,
    ),
    app("people.set_status", "Set Status…", category::PEOPLE),
    app("people.view_profile", "View Profile…", category::PEOPLE),
    app("people.message", "Message Someone…", category::PEOPLE),
    app("app.sign_out", "Sign Out…", category::APP),
    app("app.check_for_updates", "Check for Updates", category::APP),
    app("app.restart_to_update", "Restart to Update", category::APP),
    app("app.open_log_folder", "Open Log Folder", category::APP),
    app("app.minimize_window", "Minimize Window", category::APP),
    app(
        "app.toggle_maximize_window",
        "Maximize or Restore Window",
        category::APP,
    ),
    app("app.close_window", "Close Window", category::APP),
    app("settings.open.profile", "Settings: Profile", category::APP),
    app("settings.open.account", "Settings: Account", category::APP),
    app(
        "settings.open.security",
        "Settings: Security",
        category::APP,
    ),
    app("settings.open.desktop", "Settings: Desktop", category::APP),
    app(
        "settings.open.contacts",
        "Settings: Contacts",
        category::APP,
    ),
    app(
        "settings.open.messages",
        "Settings: Messages",
        category::APP,
    ),
    app(
        "settings.open.direct_messages",
        "Settings: Direct messages",
        category::APP,
    ),
    app(
        "settings.open.notifications",
        "Settings: Notifications",
        category::APP,
    ),
    app("settings.open.history", "Settings: History", category::APP),
    app(
        "settings.open.keybinds",
        "Settings: Keybinds",
        category::APP,
    ),
    app(
        "settings.open.keyboard",
        "Settings: Keyboard",
        category::APP,
    ),
    app(
        "settings.open.commands",
        "Settings: Commands",
        category::APP,
    ),
    app("settings.open.macros", "Settings: Macros", category::APP),
    app(
        "settings.open.pipelines",
        "Settings: Pipelines",
        category::APP,
    ),
    app("settings.open.plugins", "Settings: Plugins", category::APP),
    app(
        "settings.open.plugin_ai",
        "Settings: Assistant's Plugin Commands",
        category::APP,
    ),
    app("settings.open.state", "Settings: State", category::APP),
    app("settings.open.emoji", "Settings: Emoji", category::APP),
    app("settings.open.agents", "Settings: Agents", category::APP),
    app("settings.open.agent", "Settings: Assistant", category::APP),
    app(
        "settings.open.workspace",
        "Settings: Workspace",
        category::WORKSPACE,
    ),
    app(
        "settings.open.workspace_moderation",
        "Settings: Moderation",
        category::WORKSPACE,
    ),
    app(
        "settings.open.bots",
        "Settings: Bots & webhooks",
        category::WORKSPACE,
    ),
    app(
        "settings.open.moderation",
        "Settings: Instance moderation",
        category::APP,
    ),
    app("settings.open.about", "Settings: About", category::APP),
    app("settings.assign_keybind", "Assign Keybind…", category::APP),
    app(
        "settings.back_up_history",
        "Back Up History Now",
        category::APP,
    ),
    app("app.run_macro", "Run Macro…", category::APP),
    app(
        "app.toggle_pipeline",
        "Turn Pipeline On or Off…",
        category::APP,
    ),
    app("chat.start_reply", "Reply", category::MESSAGES),
    app("chat.start_edit", "Edit Message", category::MESSAGES),
    app(
        "chat.forward_message",
        "Forward Message…",
        category::MESSAGES,
    ),
    app(
        "chat.delete_attachment",
        "Delete Attachment…",
        category::MESSAGES,
    ),
    app("chat.attach_file", "Attach File…", category::MESSAGES),
    app("chat.insert_emoji", "Insert Emoji…", category::MESSAGES),
    app(
        "chat.select_next_message",
        "Select Next Message",
        category::MESSAGES,
    ),
    app(
        "chat.select_previous_message",
        "Select Previous Message",
        category::MESSAGES,
    ),
    app(
        "chat.select_first_message",
        "Select First Message",
        category::MESSAGES,
    ),
    app(
        "chat.select_last_message",
        "Select Last Message",
        category::MESSAGES,
    ),
    app(
        "chat.toggle_fold",
        "Fold or Unfold Message",
        category::MESSAGES,
    ),
    app(
        "chat.open_invite",
        "Invite People…",
        category::CONVERSATIONS,
    ),
    app(
        "chat.browse_channels",
        "Browse Channels…",
        category::CONVERSATIONS,
    ),
    app(
        "chat.accept_invitation",
        "Accept Invitation",
        category::CONVERSATIONS,
    ),
    app(
        "chat.decline_invitation",
        "Decline Invitation",
        category::CONVERSATIONS,
    ),
    app(
        "chat.mark_conversation_unread",
        "Mark Conversation Unread",
        category::CONVERSATIONS,
    ),
    app("chat.apply_lens", "Apply Lens…", category::CONVERSATIONS),
    app("chat.clear_lens", "Clear Lens", category::CONVERSATIONS),
    app(
        "notifications.mark_read",
        "Mark Notification Read",
        category::NOTIFICATIONS,
    ),
    app(
        "unreads.mark_all_shown",
        "Unreads: Mark All Shown as Read",
        category::UNREADS,
    ),
    app(
        "unreads.toggle_muted",
        "Unreads: Show or Hide Muted",
        category::UNREADS,
    ),
    app(
        "agents.allow_in_encrypted_conversation",
        "Let the Assistant Read This Conversation…",
        category::AGENTS,
    ),
    app(
        "moderation.open_reports",
        "Open Reports",
        category::MODERATION,
    ),
    app(
        "moderation.resolve_report",
        "Resolve Report…",
        category::MODERATION,
    ),
    app(
        "moderation.dismiss_report",
        "Dismiss Report…",
        category::MODERATION,
    ),
    app("moderation.clear_flag", "Clear Flag", category::MODERATION),
    app(
        "moderation.block_principal",
        "Block Account…",
        category::MODERATION,
    ),
    app(
        "moderation.unblock_principal",
        "Unblock Account…",
        category::MODERATION,
    ),
    app(
        "moderation.unblock_ip",
        "Unblock Address…",
        category::MODERATION,
    ),
    app(
        "moderation.clear_profile",
        "Clear Profile…",
        category::MODERATION,
    ),
    app(
        "workspace.revoke_invite",
        "Revoke Invite…",
        category::WORKSPACE,
    ),
    app(
        "workspace.accept_join_request",
        "Accept Join Request…",
        category::WORKSPACE,
    ),
    app(
        "workspace.decline_join_request",
        "Decline Join Request…",
        category::WORKSPACE,
    ),
    app("bots.new", "New Bot…", category::BOTS_AND_WEBHOOKS),
    app("bots.edit", "Edit Bot…", category::BOTS_AND_WEBHOOKS),
    app(
        "bots.deactivate",
        "Deactivate Bot…",
        category::BOTS_AND_WEBHOOKS,
    ),
    app(
        "bots.new_webhook",
        "New Webhook…",
        category::BOTS_AND_WEBHOOKS,
    ),
    app(
        "bots.revoke_webhook",
        "Revoke Webhook…",
        category::BOTS_AND_WEBHOOKS,
    ),
    app("plugins.install", "Install Plugin…", category::PLUGINS),
    app(
        "plugins.toggle",
        "Turn Plugin On or Off…",
        category::PLUGINS,
    ),
    app("plugins.update", "Update Plugin…", category::PLUGINS),
    app("plugins.uninstall", "Uninstall Plugin…", category::PLUGINS),
    app(
        "plugins.revoke_ai_access",
        "Revoke a Plugin's AI Access…",
        category::PLUGINS,
    ),
    app("people.add_contact", "Add to Contacts", category::PEOPLE),
    app(
        "people.remove_contact",
        "Remove from Contacts",
        category::PEOPLE,
    ),
    app("people.block", "Block…", category::PEOPLE),
    app("people.unblock", "Unblock…", category::PEOPLE),
    app(
        "people.accept_request",
        "Accept Message Request",
        category::PEOPLE,
    ),
    app(
        "people.ignore_request",
        "Ignore Message Request",
        category::PEOPLE,
    ),
    app(
        "people.report_request",
        "Report Message Request…",
        category::PEOPLE,
    ),
    app(
        "people.read_request_first",
        "Read Message Request First",
        category::PEOPLE,
    ),
    app(
        "agents.approve_request",
        "Approve the Agent's Request…",
        category::AGENTS,
    ),
    app(
        "agents.refuse_request",
        "Refuse the Agent's Request",
        category::AGENTS,
    ),
    app(
        "agents.approve_pairing",
        "Connect the Waiting Agent…",
        category::AGENTS,
    ),
    app(
        "agents.refuse_pairing",
        "Don't Connect the Waiting Agent",
        category::AGENTS,
    ),
    app("agents.disconnect", "Disconnect Agent…", category::AGENTS),
    app(
        "agents.choose_reads",
        "Choose What an Agent Can Read…",
        category::AGENTS,
    ),
    app(
        "agents.choose_plugin_commands",
        "Choose an Agent's Commands…",
        category::AGENTS,
    ),
    app("agents.open_runs", "Open Agent Runs", category::AGENTS),
    app(
        "agents.connect_agent",
        "Connect an Agent…",
        category::AGENTS,
    ),
    app(
        "agents.start_session",
        "Start Agent Session…",
        category::AGENTS,
    ),
    app(
        "agents.stop_session",
        "Stop Agent Session",
        category::AGENTS,
    ),
    app(
        "agents.ask_about_message",
        "Ask Agent About Message…",
        category::AGENTS,
    ),
    app(
        "agents.ask_about_conversation",
        "Ask Agent About Conversation…",
        category::AGENTS,
    ),
    app(
        "agents.open_record",
        "What Agents Were Given",
        category::AGENTS,
    ),
    app(
        "agents.clear_record",
        "Clear the Agents Record…",
        category::AGENTS,
    ),
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

/// The app commands that approve, refuse, connect, start or manage an outside agent: no macro
/// step and no plugin runs one.
pub(crate) const AGENT_MANAGEMENT_COMMANDS: &[&str] = &[
    "agents.approve_request",
    "agents.refuse_request",
    "agents.approve_pairing",
    "agents.refuse_pairing",
    "agents.disconnect",
    "agents.choose_reads",
    "agents.choose_plugin_commands",
    "agents.open_runs",
    "agents.open_record",
    "agents.clear_record",
    "agents.connect_agent",
    "agents.start_session",
    "agents.stop_session",
    "agents.ask_about_message",
    "agents.ask_about_conversation",
    "workspace.open_agent_sessions",
];

/// Whether `command_id` is one of [`AGENT_MANAGEMENT_COMMANDS`].
pub(crate) fn is_agent_management_command(command_id: &str) -> bool {
    AGENT_MANAGEMENT_COMMANDS.contains(&command_id)
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

    /// Each settings tab command's id is `settings.open.` and a key of lowercase letters, digits
    /// and single underscores, and its title starts with `Settings: `.
    #[test]
    fn settings_tab_commands_follow_the_naming_rule() {
        let tabs: Vec<&AppCommand> = APP_COMMANDS
            .iter()
            .filter(|command| command.id.starts_with("settings.open."))
            .collect();
        assert_eq!(tabs.len(), 25);
        for command in tabs {
            let key = &command.id["settings.open.".len()..];
            assert!(
                !key.is_empty()
                    && !key.starts_with('_')
                    && !key.ends_with('_')
                    && !key.contains("__")
                    && key
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{}",
                command.id
            );
            assert!(command.title.starts_with("Settings: "), "{}", command.id);
            assert!(
                [category::APP, category::WORKSPACE].contains(&command.category),
                "{}",
                command.id
            );
        }
    }

    /// Every agent-management id is an app command in category Agents.
    #[test]
    fn agent_management_commands_are_app_commands() {
        assert_eq!(AGENT_MANAGEMENT_COMMANDS.len(), 16);
        for id in AGENT_MANAGEMENT_COMMANDS {
            let definition = app_command(id).unwrap_or_else(|| panic!("{id} is an app command"));
            assert_eq!(definition.category, category::AGENTS, "{id}");
        }
        for id in AGENT_APPROVAL_IDS {
            assert!(is_agent_management_command(id), "{id}");
        }
        assert!(!is_agent_management_command(
            "agents.allow_in_encrypted_conversation"
        ));
    }

    /// The approval ids are app commands in category Agents: never a builtin the server lists,
    /// never AI-callable, and run only by the client.
    #[test]
    fn agent_approval_commands_are_offered_to_no_agent() {
        let builtins = builtin_commands();
        let ai_callable = collect_ai_callable_inventory(&builtins);
        for id in AGENT_APPROVAL_IDS {
            assert!(!ai_callable.contains(&id), "{id}");
            assert!(builtins.iter().all(|builtin| builtin.id != id), "{id}");
            let definition = app_command(id).unwrap_or_else(|| panic!("{id} is an app command"));
            assert!(!definition.ai_callable, "{id}");
            assert!(!definition.visible_in_palette, "{id}");
            assert_eq!(
                definition.execution_target,
                CommandExecutionTargetDto::DesktopLocal,
                "{id}"
            );
            assert_eq!(definition.category, category::AGENTS, "{id}");
        }
    }
}
