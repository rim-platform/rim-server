//! The fixed categories a built-in command is in, whether the server lists it or it is an app
//! command. A plugin's commands, user commands and macros have categories of their own.

/// Moving between places: back and forward, conversations, workspaces, pane focus.
pub(crate) const NAVIGATION: &str = "Navigation";
/// Acting on one message.
pub(crate) const MESSAGES: &str = "Messages";
/// Channels, groups and direct messages as a whole.
pub(crate) const CONVERSATIONS: &str = "Conversations";
/// The search box, search in a conversation, message search.
pub(crate) const SEARCH: &str = "Search";
/// Contacts, blocking, profiles, status, direct message requests.
pub(crate) const PEOPLE: &str = "People";
/// Home and its edit mode.
pub(crate) const HOME: &str = "Home";
/// The Unreads page.
pub(crate) const UNREADS: &str = "Unreads";
/// The inbox, do not disturb, levels, mutes.
pub(crate) const NOTIFICATIONS: &str = "Notifications";
/// Owner and moderator actions.
pub(crate) const MODERATION: &str = "Moderation";
/// The workspace itself.
pub(crate) const WORKSPACE: &str = "Workspace";
/// The assistant built into RIM, and agents on your computer.
pub(crate) const AGENTS: &str = "Agents";
/// Bots, webhooks, the channel's bots panel.
pub(crate) const BOTS_AND_WEBHOOKS: &str = "Bots & Webhooks";
/// Managing plugins.
pub(crate) const PLUGINS: &str = "Plugins";
/// Layout and appearance.
pub(crate) const VIEW: &str = "View";
/// The app: settings, quitting, updates, signing out, reloading commands, the window.
pub(crate) const APP: &str = "App";
/// The `state.*` commands.
pub(crate) const STATE: &str = "State";

/// Every category a built-in command may be in.
#[cfg(test)]
pub(crate) const ALL: [&str; 16] = [
    NAVIGATION,
    MESSAGES,
    CONVERSATIONS,
    SEARCH,
    PEOPLE,
    HOME,
    UNREADS,
    NOTIFICATIONS,
    MODERATION,
    WORKSPACE,
    AGENTS,
    BOTS_AND_WEBHOOKS,
    PLUGINS,
    VIEW,
    APP,
    STATE,
];
