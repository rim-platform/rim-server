# Command Model

This document defines the command model for RIM.

The command system is one of the core differentiators of the product. Commands are the backbone for:

- built-in workflows
- user-defined workflows
- keybinds
- plugin-provided actions
- context actions
- automations

## Purpose

A command is a named, invokable action with:

- a stable id
- declared arguments
- declared context requirements
- declared capabilities
- a defined execution target
- a structured result

Commands should be the common execution unit across the product.

This means:

- the command palette invokes commands
- keybinds invoke commands
- context actions invoke commands
- plugins register commands
- automations may invoke commands
- macros, a person's own actions made of commands, are commands too

## Current Backend State

The current backend/contracts implementation already provides:

- built-in command definitions
- command listing and single-command lookup
- backend command execution
- persisted macros, kept with the person's account on their home server
- macro create/update/delete flows
- command context-aware filtering for palette/context-menu/keybind use cases

A macro's steps may name any command: a built-in the server lists, an app command or a plugin
command. The client runs a macro step by step; the server runs one itself only when every step
is a built-in it executes, with every argument fixed (Commands And Macros, below).

## Product Requirements

The command system must be:

- easy to understand for end users
- easy to extend for plugin authors
- easy to wire into keybinds and context actions
- safe by default
- explicit about dangerous local behavior

## Command Identity

Every command must have a stable, namespaced id.

Examples:

- `chat.post_message`
- `chat.open_thread`
- `workspace.open_panel`
- `integration.youtrack.create_issue`
- `plugin.k8s.restart_pod`
- `user.clone_repo_and_open`

Namespacing rules:

- built-in commands use product namespaces like `chat.*`, `workspace.*`, `state.*`
- integration commands should use `integration.<name>.*`
- plugin commands should use `plugin.<plugin_id>.*`
- macros, the person's own commands, use `user.*`

A built-in id never changes once it is released: keybinds, Home quick boxes, macros and
pipelines store it. What a command means to a person comes from its title and its
category, not from its id, so a released id whose namespace no longer fits stays as it is:
`workspace.next_channel` moves through every conversation and is titled Next Conversation.

A new built-in, whether the server lists it or it is an app command (below), takes the
namespace of its feature:

| Namespace | Feature |
|---|---|
| `chat.` | Messages and conversations |
| `workspace.` | The workspace itself |
| `home.` | Home |
| `unreads.` | The Unreads page |
| `notifications.` | The notification inbox, do not disturb, levels and mutes |
| `people.` | Contacts, blocking, profiles, status and direct message requests |
| `moderation.` | Owner and moderator actions |
| `agents.` | Outside agents and the in-app AI agent |
| `bots.` | Bots and webhooks |
| `plugins.` | Managing plugins |
| `settings.` | The settings pages |
| `app.` | The app: its window, updates, signing out and quitting |

## Categories

Every command has a category, which groups it in the palette, the keybind picker and every
other list a client shows. A built-in command, listed by the server or an app command, is in one
of these and no other:

| Category | What goes in it |
|---|---|
| Navigation | Moving between places: back and forward, next and previous conversation, jumping to a conversation, switching workspace, pane focus |
| Messages | Acting on one message: post, reply, edit, delete, pin, react, copy, thread, mark unread from here, scroll |
| Conversations | Channels, groups and direct messages as a whole: create, join, leave, invite, members, the pinned panel, topic, mark read, browse |
| Search | The search box, search in a conversation, message search |
| People | Contacts, blocking, profiles, status, direct message requests |
| Home | Home and its edit mode |
| Unreads | The Unreads page |
| Notifications | The inbox, do not disturb, levels, mutes |
| Moderation | Owner and moderator actions: kick, ban, roles, timeouts, slow mode, who can post, archive, reports |
| Workspace | The workspace itself: settings, rename, leave, invites, the member directory, the audit log, adding a workspace |
| Agents | Outside agents and the in-app AI agent |
| Bots & Webhooks | Bots, webhooks, the channel's bots panel |
| Plugins | Managing plugins |
| View | Layout and appearance: sidebar, zen mode, density, layout preset, vim mode, the shortcuts list, the palette, closing a panel |
| App | The app: settings, quitting, updates, signing out, reloading commands, the window |
| State | The `state.*` commands |

A plugin's own commands are in a category named after the plugin's display name. A macro is in
`Macro`.

## Command Kinds

RIM should support the following command sources.

### Built-in Commands

- Shipped by RIM.
- Stable and documented.
- Usually available in the command palette.

Channel builtins (`docs/messaging.md`, Channel Visibility And Membership). Apart from the read-only
`chat.list_channels` (`Safe`, `chat.read`), all are `Backend`, `Mutating`, `chat.write`, palette-
and keybind-visible. `conversation_id`
falls back to the focused conversation (`context.conversation_id`). Authorization (membership,
private-channel visibility, Owner/Moderator) lives in `ConversationService`, so the agent can
never do more than the user it acts for.

| Id | Args | AI-callable | Dangerous | Notes |
|---|---|---|---|---|
| `chat.list_channels` | `server_id` | yes | no | Read; each channel is `{id, title, conversation_type, is_private, is_archived, is_member}` (`is_member` is the caller's; member ids are never returned). Default filter: every visible, unarchived channel |
| `chat.create_channel` | `server_id`, `name`, `description?`, `is_private?` (default false), `member_ids?` | yes | no | Members are added right after creation; privacy cannot change later |
| `chat.join_channel` | `server_id`, `conversation_id` | yes | no | Public, unarchived channels only; idempotent |
| `chat.leave_conversation` | `server_id`, `conversation_id` | yes | no | Channels and groups, not DMs |
| `chat.invite_to_conversation` | `server_id`, `conversation_id`, `user_ids` | yes | no | Caller must be a member |
| `chat.archive_channel` | `server_id`, `conversation_id` | yes | no | Owner/Moderator |
| `chat.unarchive_channel` | `server_id`, `conversation_id` | yes | no | Owner/Moderator |
| `chat.remove_conversation_member` | `server_id`, `conversation_id`, `user_id` | no | yes | Owner/Moderator, or yourself; confirmation required |

Pin and topic builtins. Both are `Backend`, `Mutating`, not dangerous, need `chat.write` and,
like every `Mutating` command, the agent's confirmation. `conversation_id` falls back to the
focused conversation. They call the service methods of the GraphQL mutations they mirror, which
hold the rules (`docs/messaging.md`, Channel Visibility And Membership), so the command allows
exactly what the mutation allows.

| Id | Args | AI-callable | Category | Notes |
|---|---|---|---|---|
| `chat.unpin_message` | `server_id`, `conversation_id`, `message_id` | yes | Messages | Mirrors `unpinMessage` (`PinService`). Anyone who may write unpins what they pinned; a message someone else pinned in a channel is unpinned by an Owner, a Moderator or a private channel's creator (`FORBIDDEN` otherwise). `message_id` falls back to the selected message. Palette, not keybind, like `chat.pin_message`. Payload `{success, unpinned}` |
| `chat.set_topic` | `server_id`, `conversation_id`, `topic` | yes | Conversations | Title Edit Topic…. Mirrors `updateConversationTopic` (`ConversationService`): a channel's topic is changed by an Owner, a Moderator or a private channel's creator, a DM's or group's by any member (`FORBIDDEN` otherwise); archived conversations refuse it. `topic` is trimmed, control characters other than newlines are dropped, and an empty `topic` clears it; more than 512 characters is `VALIDATION`. Palette and keybind. Payload `{conversation_id, topic}` (`topic` null when cleared) |

Moderation builtins. All are
`Backend` and `Mutating`; the `admin.*` ones require `admin.manage_members`, the `chat.*` ones
`chat.write`. They call `ModerationService`, which enforces the roles (Owner/Moderator, who may
target whom) and writes the moderation audit row (`docs/messaging.md`, Roles And Moderation), so
a command execution is audited twice: the generic command row plus the moderation row.
`chat.delete_message` (context menu, selected message) goes through `MessageService` as before,
which now lets an Owner/Moderator delete other people's messages.

| Id | Args | AI-callable | Dangerous | Notes |
|---|---|---|---|---|
| `admin.kick` | `server_id`, `user_id`, `reason?` | no | yes | Owner/Moderator; confirmation required |
| `admin.ban` | `server_id`, `user_id`, `reason?`, `expires_at?` (RFC 3339) | no | yes | Owner/Moderator; confirmation required |
| `admin.unban` | `server_id`, `user_id` | no | no | Owner/Moderator |
| `admin.timeout` | `server_id`, `user_id`, `minutes` (1..=43200), `reason?` | yes | no | Owner/Moderator |
| `admin.clear_timeout` | `server_id`, `user_id` | yes | no | Mirrors `clearTimeout`: Owner/Moderator, with the targeting rules of `admin.timeout`. Category Moderation; palette, offered to Owners and Moderators, and keybind. Clearing a timeout in force is audited and emits `MemberTimeoutChanged`; clearing none succeeds and changes nothing. Payload `{user_id, cleared}` |
| `admin.set_role` | `server_id`, `user_id`, `role` (`moderator`/`member`/`guest`) | no | yes | Owner: any but owner; Moderator: member/guest; confirmation required |
| `admin.delete_member_messages` | `server_id`, `user_id`, `window` (`1h`/`24h`/`7d`/`all`), `include_unjoined_private?` (default false) | no | yes | Owner/Moderator; target need not be a member; at most 1000 per run; confirmation required |
| `chat.set_slow_mode` | `server_id`, `conversation_id`, `seconds` (0..=21600) | yes | no | Owner/Moderator; channels only; `conversation_id` falls back to the focused conversation |
| `chat.set_channel_posting` | `server_id`, `conversation_id`, `policy` (`everyone`/`moderators`), `allows_threads?` (default true) | yes | no | Owner/Moderator; channels only; `conversation_id` falls back to the focused conversation |
| `chat.report_message` | `message_id?`, `reason` | yes | no | Needs `workspace` and `conversation` context; `message_id` falls back to the selected message. Offered in a context menu with a selected message |
| `chat.mark_as_unread` | `server_id`, `conversation_id`, `message_id` | no | no | `Mutating`, needs no capability: it changes only the caller's own read state. Palette, context menu and keybind. `conversation_id` falls back to the focused conversation and `message_id` to the selected message. The message and later ones count as unread (`docs/messaging.md`) |

Notification builtins (`docs/messaging.md`, Notification Rules). All are
`Backend`, `Mutating`, palette- and keybind-visible, AI-callable, in category `Notifications`,
and need **no capability**: like `chat.mark_as_read` they change only the caller's own state.
They call `NotificationService`, or for the mutes `NotificationPreferenceService` (the service
of `muteConversation` and `unmuteConversation`), which require workspace membership (and, for
the level and the mutes, read access to the conversation). Being `Mutating`, they are refused
while the caller is timed out (T1); the GraphQL mutations are not.

| Id | Args | AI-callable | Dangerous | Notes |
|---|---|---|---|---|
| `notifications.set_level` | `server_id`, `conversation_id`, `level` (`all`/`mentions`/`none`/`default`) | yes | no | `default` clears the override; `conversation_id` falls back to the focused conversation. Payload `{conversation_id, level, effective_level}` |
| `notifications.dnd` | `server_id`, `mode` (`on`/`off`/`until`), `until?` (RFC 3339), `minutes?` (1..=10080) | yes | no | The manual do-not-disturb pause (separate from the daily window); `until` mode needs `until` or `minutes`. Payload `{paused, until}` |
| `notifications.mark_all_read` | `server_id` | yes | no | Marks every notification of the workspace read (not the conversations' unread counts). Payload `{marked}` |
| `notifications.mute_conversation` | `server_id`, `conversation_id`, `duration` (`1h`/`8h`/`until_tomorrow`/`forever`/`until`), `until?` (RFC 3339) | yes | no | Title Mute Conversation…. Mirrors `muteConversation`; `conversation_id` falls back to the focused conversation. `1h` and `8h` count from now. `until_tomorrow` ends at 09:00 tomorrow in the timezone of the caller's daily do-not-disturb window in this workspace, or in UTC when they have none: the server knows no other timezone of theirs. `until` ends at the `until` time, which must be in the future, and `until` with another duration is `VALIDATION`; a client that knows the person's timezone sends it to end the mute at 09:00 their time. `forever` is `muteConversation` without `mutedUntil`: level `none` with no end. Payload `{conversation_id, muted_until}` (`muted_until` null for `forever`) |
| `notifications.unmute_conversation` | `server_id`, `conversation_id` | yes | no | Mirrors `unmuteConversation`: ends a timed mute and clears a `none` level; `conversation_id` falls back to the focused conversation. Unmuting a conversation that is not muted succeeds. Payload `{conversation_id}` |

General builtins the agent may call. None declares a new capability. The navigation
commands are `DesktopLocal`: the server refuses to execute them, so the desktop agent runs them
through the client's local handlers, and only when they are `Safe` and not dangerous (anything
stricter goes to `executeCommand`, which refuses it). The five writes were raised from `Safe` to
`Mutating`, which has three effects beyond the agent's confirmation
gate: a timed-out member can no longer run them through the command path (T1; the GraphQL
`markAsRead` mutation is not affected), server-side pipelines (initiator `Plugin`) can no longer
run them because nobody is there to confirm, and they count towards the agent's per-turn
mutating-call cap. Palette and keybind still run them in one step.

| Id | Args | Sensitivity | Agent confirmation | Notes |
|---|---|---|---|---|
| `workspace.open_search`, `workspace.search_in_channel`, `workspace.open_marketplace`, `workspace.toggle_sidebar`, `workspace.previous_channel`, `workspace.next_channel`, `workspace.next_unread_channel`, `workspace.close_panel`, `workspace.reopen_last_thread`, `chat.scroll_to_bottom`, `chat.scroll_to_top` | none | `Safe` | no | `DesktopLocal`, run by the client |
| `state.get` | `server_id`, `key`, `scope?` | `Safe` | no | Read |
| `state.list` | `server_id`, `prefix?`, `scope?`, `limit?` | `Safe` | no | Read |
| `state.set` | `server_id`, `key`, `value`, `scope?`, `ttl?` | `Mutating` | yes | |
| `state.incr` | `server_id`, `key`, `delta?`, `scope?` | `Mutating` | yes | |
| `state.delete` | `server_id`, `key`, `scope?` | `Mutating` | yes | |
| `chat.mark_as_read` | `server_id`, `conversation_id` | `Mutating` | yes | `conversation_id` falls back to the focused conversation |
| `chat.create_group` | `server_id`, `member_ids`, `title?` | `Mutating` | yes | `member_ids` is a JSON array or one comma-separated string |

The `state.*` commands and the GraphQL roots `workspaceState`, `workspaceStateList`,
`setWorkspaceState` and `deleteWorkspaceState` follow the same rules. Every member reads the
workspace scope (`scope: server`) and reads and writes their own user scope. Writing the
workspace scope takes a Member, Moderator or Owner who is not timed out and whose account is out
of probation; a guest gets `FORBIDDEN`. A Member overwrites and deletes only the keys they
created and increments any numeric key, so a shared counter takes everyone's count; Owners and
Moderators change every key. A key whose entry expired is written as a new entry of its writer,
even before the expired row is purged. A Member creates at most 100 workspace-scoped entries per
workspace, holding at most 1 MiB of values; a workspace holds at most 10,000, a user scope
1,000, and a value at most 64 KiB. Each user makes at most 60 writes (sets, increments and
deletes, either scope) a minute in bursts of up to 30, however they reach the service (GraphQL,
a command, a macro or pipe step, a pipeline); past that a write answers `RATE_LIMITED` with
`retryAfterSeconds`. `WorkspaceStateChanged` names the key, scope and version
but not the value, which a client reads again.

Data the agent sends to its model provider grows with this set: `state.get` and `state.list` return
stored values, including the caller's own `scope: user` entries, and nothing redacts secrets kept
in workspace state. Kept off the AI surface on purpose: `chat.edit_message`,
`chat.delete_message`, `chat.reply_to_selected_message` and `chat.copy_message_text` (they act on
the user's selected message, which the agent cannot supply), `admin.invite_member` (its result is
an invite token that would pass through the model provider), `chat.mark_all_as_read` (it marks
every conversation of the workspace read at once, which is the person's call; it is not
`notifications.mark_all_read`, which marks notifications and which the agent may call), and
`chat.mark_as_unread` (an agent does not decide what someone has read).

List arguments such as `user_ids` and `member_ids` accept a JSON array of ids or one
comma-separated string, because the agent's tool schema types every argument as a scalar.

AI-callable builtins: 47. They are nineteen chat, channel and navigation builtins
(`chat.post_message`, `chat.open_thread`, `workspace.open_settings`,
`workspace.navigate_to_channel`, `workspace.switch_workspace`, `chat.list_channels`,
`chat.create_channel`, `chat.list_members`, `chat.search_messages`, `chat.get_recent_messages`,
`chat.pin_message`, `chat.unpin_message`, `chat.set_topic`, `chat.toggle_reaction` and the five
channel membership builtins above), `admin.timeout`, `admin.clear_timeout`,
`chat.set_slow_mode`, `chat.set_channel_posting` and `chat.report_message`, the five
`notifications.*` builtins, and the eighteen general builtins above. The set is pinned by
`collect_ai_callable_inventory_matches_reviewed_set` (`registry/mod.rs`) and
`list_commands_ai_surface_returns_forty_seven_commands_with_caps` (`command_tests.rs`). The
capability union the AI surface checks is `admin.manage_members`, `chat.read` and `chat.write`;
the notification and general builtins declare none.

Each of the five that joined the set last mirrors a command the agent could already call, is
checked by the same service, and is `Mutating`, so it waits for the person's confirmation:

- `chat.unpin_message` undoes `chat.pin_message`, and unpinning someone else's pin takes the
  role the person already has.
- `chat.set_topic` changes text members see, as `chat.post_message` does, and only for those
  who may change the topic.
- `admin.clear_timeout` lifts what `admin.timeout` sets, with the same targeting rules.
- `notifications.mute_conversation` and `notifications.unmute_conversation` change only the
  person's own notifications, as `notifications.set_level` does.

`chat.post_message`, `chat.get_recent_messages`, `chat.pin_message`, `chat.unpin_message`,
`chat.set_topic`, `chat.toggle_reaction`, `notifications.mute_conversation` and
`notifications.unmute_conversation` take `conversation_id` from the focused conversation
(`context.conversation_id`) when it is not given, and `chat.pin_message`,
`chat.unpin_message`, `chat.toggle_reaction` and `chat.report_message` take `message_id` from
the selected message (`context.selected_message_id`); an argument that is given wins. The agent
never sends a selected message, so it names the message by `message_id`.
`chat.search_messages` has no such fallback: without `conversation_id` it searches the
workspace.

`workspace.navigate_to_dm` (`DesktopLocal`, not AI-callable) takes a required
`conversation_id`, the direct message or group to open. The three navigation builtins that move
through the sidebar move through every conversation, not only channels, and are titled Previous
Conversation (`workspace.previous_channel`), Next Conversation (`workspace.next_channel`) and
Next Unread Conversation (`workspace.next_unread_channel`).

### App Commands

An app command is a built-in command the client runs itself and the server does not list:
opening a page, toggling a panel, moving focus, changing the view, quitting the app. The client
holds their definitions. The server knows their ids, so that a person's keybinds and a
workspace's keybinds can name them:

- `createKeybind` and `updateKeybind` accept an app command id as they accept the id of a
  built-in the server lists, for `user` and `workspace` keybinds alike, and `resolvedKeybinds`
  returns those keybinds, so they reach every device the person signs in on, the web app
  included. An app command changes nothing on the server, so the rule for workspace keybinds
  (`keybind_model.md`, Workspace Keybinds) admits it; a preset `server_id` must still name the
  keybind's own workspace. One the table marks as not bindable is refused like a built-in that
  is not keybind-capable.
- `commands` never lists an app command, on any surface. `executeCommand` refuses one as it
  refuses an id that names no command, and the AI surface and `commandDiscover` never offer one
  to an agent. A pipeline action cannot name one. A macro step can, and the client runs it
  (Commands And Macros, below).

A server that accepts these keybinds lists `app_command_keybinds` in `serverInfo.capabilities`.
An older server refuses them with `VALIDATION`, and a client keeps such a binding on the device
there.

| Id | Title | Category | Notes |
|---|---|---|---|
| `workspace.show_shortcuts` | Show Keyboard Shortcuts | View | |
| `workspace.toggle_vim_mode` | Toggle Vim Mode | View | |
| `workspace.previous_unread_channel` | Previous Unread Conversation | Navigation | |
| `workspace.navigate_panel_left` | Focus the Pane to the Left | Navigation | |
| `workspace.navigate_panel_right` | Focus the Pane to the Right | Navigation | |
| `workspace.cycle_layout_preset` | Cycle Layout Preset | View | |
| `workspace.toggle_zen_mode` | Toggle Zen Mode | View | |
| `workspace.back_to_workspace` | Back to Workspace | Navigation | |
| `workspace.cycle_display_density` | Cycle Display Density | View | |
| `workspace.rename` | Rename Workspace… | Workspace | Opens the rename dialog |
| `workspace.open_bots` | Open Bots | Bots & Webhooks | |
| `workspace.leave` | Leave Workspace… | Workspace | Opens the confirmation dialog |
| `workspace.open_agents` | Open Agents | Agents | Desktop only |
| `workspace.open_unreads` | Open Unreads | Unreads | |
| `workspace.unreads_next` | Unreads: Next Message | Unreads | |
| `workspace.unreads_previous` | Unreads: Previous Message | Unreads | |
| `workspace.unreads_mark_group` | Unreads: Mark Conversation as Read | Unreads | |
| `workspace.unreads_open` | Unreads: Open Message | Unreads | |
| `workspace.unreads_undo` | Unreads: Undo | Unreads | |
| `workspace.open_home` | Open Home | Home | |
| `workspace.edit_home` | Edit Home | Home | Not bindable: changing Home is the person's act from the palette or Home |
| `workspace.open_audit_log` | Open Audit Log | Workspace | |
| `workspace.open_message_requests` | Open Message Requests | People | |
| `workspace.toggle_pinned_messages` | Toggle Pinned Messages | Conversations | |
| `workspace.toggle_channel_bots` | Toggle Bots in This Channel | Bots & Webhooks | |
| `workspace.toggle_notifications` | Toggle Notifications | Notifications | |
| `workspace.toggle_conversation_members` | Toggle Conversation Members | Conversations | |
| `workspace.toggle_member_directory` | Toggle Workspace Members | Workspace | |
| `workspace.toggle_lens_picker` | Toggle Channel Lenses | Conversations | |
| `workspace.go_back` | Go Back | Navigation | |
| `workspace.go_forward` | Go Forward | Navigation | |
| `app.quit` | Quit RIM | App | Desktop only; asks to confirm |
| `plugins.open_settings` | Open Plugin Settings | Plugins | Settings, Plugins tab |
| `workspace.open_direct_messages` | Open Direct Messages | Navigation | |
| `workspace.add` | Add a Workspace… | Workspace | Opens the add dialog |
| `workspace.open_workspace_settings` | Open Workspace Settings | Workspace | Owners and moderators |
| `workspace.open_join_requests` | Open Join Requests | Workspace | Owners and moderators |
| `people.set_status` | Set Status… | People | Opens the status picker |
| `people.view_profile` | View Profile… | People | Opens a member picker |
| `people.message` | Message Someone… | People | Opens a member picker |
| `app.sign_out` | Sign Out… | App | Asks to confirm |
| `app.check_for_updates` | Check for Updates | App | Desktop only |
| `app.restart_to_update` | Restart to Update | App | Desktop only; offered while an update is ready |
| `app.open_log_folder` | Open Log Folder | App | Desktop only |
| `app.minimize_window` | Minimize Window | App | Desktop only |
| `app.toggle_maximize_window` | Maximize or Restore Window | App | Desktop only |
| `app.close_window` | Close Window | App | Desktop only; as the window's close button |
| `settings.open.profile` | Settings: Profile | App | |
| `settings.open.account` | Settings: Account | App | |
| `settings.open.security` | Settings: Security | App | |
| `settings.open.desktop` | Settings: Desktop | App | Desktop only |
| `settings.open.contacts` | Settings: Contacts | App | |
| `settings.open.messages` | Settings: Messages | App | |
| `settings.open.direct_messages` | Settings: Direct messages | App | |
| `settings.open.notifications` | Settings: Notifications | App | |
| `settings.open.history` | Settings: History | App | |
| `settings.open.keybinds` | Settings: Keybinds | App | |
| `settings.open.keyboard` | Settings: Keyboard | App | |
| `settings.open.commands` | Settings: Commands | App | |
| `settings.open.macros` | Settings: Macros | App | |
| `settings.open.pipelines` | Settings: Pipelines | App | Not in the direct space |
| `settings.open.plugins` | Settings: Plugins | App | |
| `settings.open.plugin_ai` | Settings: Plugin AI | App | Desktop only |
| `settings.open.state` | Settings: State | App | Not in the direct space |
| `settings.open.emoji` | Settings: Emoji | App | Where the server has custom emoji; not in the direct space |
| `settings.open.agents` | Settings: Agents | App | Desktop only |
| `settings.open.agent` | Settings: AI Agent | App | Desktop only |
| `settings.open.workspace` | Settings: Workspace | Workspace | Owners and moderators; not in the direct space |
| `settings.open.workspace_moderation` | Settings: Moderation | Workspace | Owners and moderators; not in the direct space |
| `settings.open.bots` | Settings: Bots | Workspace | Those who manage the workspace's bots; not in the direct space |
| `settings.open.moderation` | Settings: Instance moderation | App | Instance operators |
| `settings.open.about` | Settings: About | App | |
| `settings.assign_keybind` | Assign Keybind… | App | Opens the keybind assigner for the focused conversation, else the open workspace |
| `settings.back_up_history` | Back Up History Now | App | Desktop only |
| `app.run_macro` | Run Macro… | App | Opens a macro picker |
| `app.toggle_pipeline` | Turn Pipeline On or Off… | App | Opens a pipeline picker |
| `chat.start_reply` | Reply | Messages | The selected message |
| `chat.start_edit` | Edit Message | Messages | Your own selected message |
| `chat.forward_message` | Forward Message… | Messages | Opens the forward dialog |
| `chat.delete_attachment` | Delete Attachment… | Messages | Asks to confirm |
| `chat.attach_file` | Attach File… | Messages | Opens the file picker |
| `chat.insert_emoji` | Insert Emoji… | Messages | Opens the emoji picker |
| `chat.select_next_message` | Select Next Message | Messages | |
| `chat.select_previous_message` | Select Previous Message | Messages | |
| `chat.select_first_message` | Select First Message | Messages | |
| `chat.select_last_message` | Select Last Message | Messages | |
| `chat.toggle_fold` | Fold or Unfold Message | Messages | |
| `chat.open_invite` | Invite People… | Conversations | Opens the invite dialog |
| `chat.browse_channels` | Browse Channels… | Conversations | Opens the browse dialog |
| `chat.accept_invitation` | Accept Invitation | Conversations | |
| `chat.decline_invitation` | Decline Invitation | Conversations | |
| `chat.mark_conversation_unread` | Mark Conversation Unread | Conversations | |
| `chat.apply_lens` | Apply Lens… | Conversations | Channels; opens the lens picker |
| `chat.clear_lens` | Clear Lens | Conversations | |
| `notifications.mark_read` | Mark Notification Read | Notifications | The inbox's selected notification |
| `unreads.mark_all_shown` | Unreads: Mark All Shown as Read | Unreads | Can be undone |
| `unreads.toggle_muted` | Unreads: Show or Hide Muted | Unreads | |
| `agents.allow_in_encrypted_conversation` | Let the AI Agent Read This Conversation… | Agents | Desktop only; always shows the warning and asks to confirm |
| `moderation.open_reports` | Open Reports | Moderation | Owners and moderators |
| `moderation.resolve_report` | Resolve Report… | Moderation | Owners and moderators; opens a report picker |
| `moderation.dismiss_report` | Dismiss Report… | Moderation | Owners and moderators; opens a report picker |
| `moderation.clear_flag` | Clear Flag | Moderation | Owners and moderators; the selected flagged message |
| `moderation.block_principal` | Block Principal… | Moderation | Instance operators; asks to confirm |
| `moderation.unblock_principal` | Unblock Principal… | Moderation | Instance operators; opens a picker |
| `moderation.unblock_ip` | Unblock Address… | Moderation | Instance operators; opens a picker |
| `moderation.clear_profile` | Clear Profile… | Moderation | Instance operators; asks to confirm |
| `workspace.revoke_invite` | Revoke Invite… | Workspace | Opens an invite picker; asks to confirm |
| `workspace.accept_join_request` | Accept Join Request… | Workspace | Owners and moderators; opens a request picker |
| `workspace.decline_join_request` | Decline Join Request… | Workspace | Owners and moderators; opens a request picker |
| `bots.new` | New Bot… | Bots & Webhooks | Opens the form |
| `bots.edit` | Edit Bot… | Bots & Webhooks | Opens a bot picker |
| `bots.deactivate` | Deactivate Bot… | Bots & Webhooks | Opens a bot picker; asks to confirm |
| `bots.new_webhook` | New Webhook… | Bots & Webhooks | The focused channel; opens the dialog |
| `bots.revoke_webhook` | Revoke Webhook… | Bots & Webhooks | Opens a webhook picker; asks to confirm |
| `plugins.install` | Install Plugin… | Plugins | Asks for consent as installing does |
| `plugins.toggle` | Turn Plugin On or Off… | Plugins | Opens a plugin picker |
| `plugins.update` | Update Plugin… | Plugins | Opens a picker of plugins with an update |
| `plugins.uninstall` | Uninstall Plugin… | Plugins | Opens a plugin picker; asks to confirm |
| `plugins.revoke_ai_access` | Revoke a Plugin's AI Access… | Plugins | Desktop only; opens a plugin picker |
| `people.add_contact` | Add to Contacts | People | The person in context, else a picker |
| `people.remove_contact` | Remove from Contacts | People | The person in context, else a picker |
| `people.block` | Block… | People | Opens the block dialog |
| `people.unblock` | Unblock… | People | Opens a picker of blocked people |
| `people.accept_request` | Accept Message Request | People | The open request |
| `people.ignore_request` | Ignore Message Request | People | The open request |
| `people.report_request` | Report Message Request… | People | The open request; opens the report dialog |
| `people.read_request_first` | Read Message Request First | People | The open request |
| `agents.approve_request` | Approve the Agent's Request… | Agents | Desktop only; always shows a loud warning and asks to confirm |
| `agents.refuse_request` | Refuse the Agent's Request | Agents | Desktop only; runs at once |
| `agents.approve_pairing` | Connect the Waiting Agent… | Agents | Desktop only; always shows a loud warning and asks to confirm |
| `agents.refuse_pairing` | Don't Connect the Waiting Agent | Agents | Desktop only; runs at once |
| `agents.disconnect` | Disconnect Agent… | Agents | Desktop only |
| `agents.choose_reads` | Choose What an Agent Can Read… | Agents | Desktop only |
| `agents.choose_plugin_commands` | Choose an Agent's Commands… | Agents | Desktop only |
| `agents.open_runs` | Open Agent Runs | Agents | Desktop only |
| `agents.open_record` | What Agents Were Given | Agents | Desktop only |
| `agents.clear_record` | Clear the Agents Record… | Agents | Desktop only |

The `settings.open.*` commands open one settings tab each. The id is `settings.open.` and the
tab's key with every run of characters other than `a`-`z` and `0`-`9` turned into `_`, and the
title is `Settings: ` and the tab's label. They are offered where the tab is shown. The tabs of
the Workspace section are in category Workspace, the others in App.

A new app command is added to this table, and to the server's list of app command ids, in the
same release that ships it in the client. Until a server knows an id it refuses keybinds to it,
and the client keeps them on the device.

#### Agent approval commands

The commands that approve or refuse an outside agent's request, or its pairing, are app commands
with reserved ids: `agents.approve_request`, `agents.refuse_request`, `agents.approve_pairing`
and `agents.refuse_pairing`. Approving an agent is the person's own act:

- none of them is ever `ai_callable`, and no plugin, pipe, macro or pipeline runs one
- the two that approve, `agents.approve_request` and `agents.approve_pairing`, always show a loud
  warning and ask the person to confirm, naming the agent and what it asked for, whichever
  surface started them, a keybind or a Home quick box included
- the two that refuse, `agents.refuse_request` and `agents.refuse_pairing`, run at once with no
  warning: refusing grants nothing
- a test pins all four as app commands offered on no agent surface

### Plugin-Provided Commands

- Registered by plugins.
- May be local-runtime or plugin-runtime specific.
- Must declare required capabilities.

### Integration-Provided Commands

- Registered by integrations such as bots, webhooks, or external tool connectors.
- Usually remote or backend-oriented.

### Macros

- Created by the person: a macro is their own action, made of commands.
- Its steps name built-in, app and plugin commands, each with its arguments.
- Every macro is a command, in category `Macro`.

## Commands And Macros

A **command** is one action that RIM or a plugin gives you: a built-in the server lists, an app
command the client runs, or a plugin's command. Nobody creates a command. A **macro** is a
person's own action, made of commands that run in order. Every macro is a command too: it is
in category `Macro`, it shows in the palette, the keybind picker, Home quick boxes and the
message box's `/` list, and a keybind names it.

Macros are stored as user commands, and the API keeps that name: `createUserCommand`,
`updateUserCommand`, `deleteUserCommand`, `userCommand` and `userCommands`. A macro's id is
`user.<key>`. `<key>` is one or more lowercase letters, numbers, dots, dashes or underscores;
`createUserCommand` refuses any other id with `VALIDATION` ("a user command id may use only
lowercase letters, numbers, dots, dashes or underscores after `user.`"), and macros saved before
this rule keep their ids. A client makes the key from the title.

A macro belongs to the account that created it, on that account's home server, and nobody else
sees it. Like the person's keybinds, it follows them into every workspace, on this server or
another: the client reads macros and keybinds from the home server. The commands a person runs
in a workspace, a macro's steps included, go to the server that hosts the workspace; the client
routes each call there.

`commands` lists a macro as a `Command` while it is enabled, without its steps. `userCommand(id)`
returns one of the caller's macros as a `UserCommand`, enabled or not, with its title,
description, steps in the order they run, `isEnabled`, `aiCallable`, `nlDescription` and
`intentSamples`: the fields `updateUserCommand` takes, so an edit form loads the saved macro
instead of asking for it again. It answers null for an id that is not one of the caller's
macros: a built-in id, another user's macro and an unknown one alike. `userCommands` lists the
caller's macros in the same shape, enabled or not, sorted by id.

### Steps

A macro lists 1 to 20 steps. Each step is a `commandId`, its fixed `args` and the names of the
arguments it asks for when the macro runs (`ask`). The step's command is one of:

- a built-in the server lists, one the server runs or one the client runs (`DesktopLocal`)
- an app command (App Commands, above), except the agent-management ones:
  `agents.approve_request`, `agents.refuse_request`, `agents.approve_pairing`,
  `agents.refuse_pairing`, `agents.disconnect`, `agents.choose_reads`,
  `agents.choose_plugin_commands`, `agents.open_runs`, `agents.open_record` and
  `agents.clear_record`
- a plugin command, `plugin.<pluginId>.<command>`: the plugin id has at least three segments
  of ASCII letters, digits and hyphens (`specs/plugin-protocol-v1.md`, section 2), the command's
  own id one or more segments of ASCII letters, digits, `_` and `-`, all joined by `.`. The
  server does not know which plugins a person installs, so it checks only the id's shape; the
  client says a step's plugin is not installed when the macro runs

`macroStepCommands` lists every built-in and app command a step may name, whatever the caller's
role and workspace, sorted by id; the client adds the plugins' commands it knows. Every argument
of a step's command is either fixed in `args`, filled from the context the macro runs in, or
asked for: `ask` names at most 32 arguments, each 1 to 64 bytes, none twice and none also a key
of `args`.

`createUserCommand` and `updateUserCommand` refuse a step they cannot accept with `VALIDATION`:
"unknown command step `<id>`" for an id that names none of the above, "macro steps must not
manage agents (`<id>`)" for an agent-management command, and "a step may ask for at most 32
arguments", "an asked argument name may be 1 to 64 bytes", "a step asks for `<name>` twice" or
"a step both fixes and asks for `<name>`" for its `ask`. A step omitting `ask` asks for nothing,
except on `updateUserCommand`, where it keeps what the stored step at the same position asked
for when that step names the same command.

A step names another macro only where it did before macros took app and plugin steps:
`createUserCommand` refuses a `user.*` step with "macro steps must not reference other macros",
and `updateUserCommand` accepts one only when the stored macro already has a step naming that
macro. Such nested macros keep working, within these limits:

- a person keeps at most 100 macros
- a chain of macros naming macros holds at most eight
- one run performs at most 100 built-in steps: a nested macro counts its own steps each time a
  step names it, and the stages of a pipe count together

`createUserCommand` and `updateUserCommand` refuse a macro over a limit with a `VALIDATION`
error that names it: "a user command may have at most 20 steps", "a user may have at most 100
user commands", "user command recursion depth exceeded" or "a user command expands to more
than 100 steps". An update is refused the same way when it would take a stored macro that
names the updated one past 100 steps. A stored macro that nests deeper than eight or expands
past 100 steps, saved before these limits, is refused when it runs, before any of its steps
runs; a pipe over 100 steps is refused with "a pipe expands to more than 100 steps". `commands`
leaves out a macro whose definition cannot be resolved (one past 100 steps, or with a step
naming an unknown or disabled command, or a cycle) and lists the rest. A disabled macro does
not run: `executeCommand` answers `NOT_FOUND` for it, as for an id that names no command.

A macro's definition adds up its steps: its required context and capabilities are those of
every step, it is dangerous when a step is, its sensitivity is the strictest of its steps', and
a keybind may name it when every step's command is keybind-capable. A plugin step counts as
keybind-capable and `Safe`; the client applies the plugin's own rules when it runs it.

### Running a macro

**The person.** The client runs the steps one by one, in order, in the context the macro runs
in. Before the first step it asks, in one form, for every argument the steps `ask` for;
cancelling the form runs nothing. A server step is its own `executeCommand` call with initiator
`MACRO`, sent to the server that hosts the workspace, with its own gates, confirmation and audit
row. An app step and a plugin step run in the client. The first step that fails or is cancelled
stops the rest, and the client says which step it was and why. A step that opens a form or a
picker does not wait for the person to finish it, so such a step goes last. Neither the client
nor the server pipes: no step receives another step's result. Only `executeCommand` with
`isPipe` (the palette's `|`) passes one command's result to the next stage.

**The server.** `executeCommand` naming `user.<key>` runs the macro on the server when its
execution target is `Backend`: every step, nested macros' steps included, is a built-in the
server runs, and no step asks for an argument. The capability and confirmation gates judge the
call once for the whole macro, each step still passes the role and timeout gates, and the call
writes one audit row. The steps run in order with their own arguments, and each step's payload
is collected under its command id. The first step that fails stops the run and its
error is the call's. A macro with an app step, a plugin step or an asked argument has the
execution target `DesktopLocal`, as has a macro naming one, and `executeCommand` refuses it
with `VALIDATION` "this macro has steps only the app runs; run it from the app", before any
step runs.

**The AI agent.** A macro runs for an agent only when its owner turned on "The AI agent can run
this" (`aiCallable`). That takes a macro whose steps are all built-ins the server runs and the
agent may run itself (`ai_callable`), with every argument fixed: no app step, no plugin step, no
other macro, no `ask`, and no built-in kept off the AI surface, such as `admin.invite_member`
or `chat.delete_message`. A macro does not reach further than the agent alone.
`createUserCommand` and `updateUserCommand` refuse `aiCallable: true` on any other macro with
`VALIDATION` "only a macro whose steps are all commands the AI agent may run, run on the server
with every argument fixed, can be run by the AI agent". `updateUserCommand` without `aiCallable`
keeps the stored value, and turns it off when the new steps no longer allow it. A stored macro
whose step stops qualifying, such as a built-in that is no longer `ai_callable`, is not offered
to the agent and its `AGENT` calls are refused, while the stored option stays as it is. The macro's `nlDescription` (its description when
empty) and `intentSamples` describe it to the agent; they are checked as a built-in's are, and a
text that looks like an instruction to the model is refused with `VALIDATION`.

The `AI` surface of `commands` and `commandDiscover` list such a macro for a workspace on its
home server where the owner may run every step. The agent runs it as one call,
`executeCommand("user.<key>")` with initiator `AGENT` on the home server: the confirmation gate
judges the macro as a whole, so a `Mutating` or dangerous step makes the call need the person's
confirmation once, and the run counts as one call against the agent's limits. A macro on
another server, or in a workspace hosted elsewhere, is not offered to the agent. `executeCommand`
with initiator `AGENT` refuses a macro that is not agent-callable, or no longer meets the rule,
with `FORBIDDEN` (refusal `DENIED`).

### Workspace macros, deprecated

Before macros were commands, a macro (`MacroDef`) was kept per workspace on the server hosting
it: `createMacro`, `updateMacro`, `deleteMacro`, `executeMacro`, `myMacros` and `macroByName`.
They are deprecated and keep working as described here until a later release removes them. The
`AI` surface and `commandDiscover` no longer offer an `ai_callable` workspace macro as
`macro.<id>`.

A client that finds `account_macros` in the home server's `serverInfo.capabilities` moves the
person's workspace macros once: for each workspace macro it creates a macro with the same title,
description and steps on the home server, and then deletes the workspace macro on the server
hosting it. A workspace macro with the same title and steps as an existing macro is only
deleted. A moved macro's own `sensitivity` is dropped: each step keeps its own gates, and the
macro's sensitivity is the strictest of its steps'. When the person reaches 100 macros the rest
stay in their workspaces. The server moves nothing itself.

A workspace macro is a list of up to 20 steps, each a builtin command id with its own
arguments; a step may not name a workspace macro or a user command. It is not a command. It runs
through `executeMacro`, its steps in order, each with its own arguments, in the context of the
run.

A user holds at most 50 workspace macros (and 50 pipelines) in a workspace, and one who is
neither Owner nor Moderator at most a fifth of `RIM_LIMIT_MACROS_PER_WORKSPACE`
(`RIM_LIMIT_PIPELINES_PER_WORKSPACE`) when it is set. In the server's default workspace, which
every account joins, that share is the only bound: the per-workspace caps do not apply there,
since macros and pipelines belong to their author and no account may fill a cap for everyone,
and an account in probation creates none (`VALIDATION` saying when it can).

`createMacro` and `updateMacro` refuse a step naming a user command with a `VALIDATION` error,
"macro steps must not reference user commands"; `executeMacro` refuses a stored workspace macro
with such a step the same way, before any of its steps runs. A step must name a builtin
command: `createMacro` and `updateMacro` refuse any other id, a plugin command included, with
`VALIDATION` "Unknown command: <id>". A step that fails stops the run: later steps do not run,
and `executeMacro` answers `ERROR` with the message `Step <n> (<title>) failed: <reason>`, where
`n` counts from 1, the title is the command's registry title (its id when the registry has none)
and the reason is the step's own error. A workspace macro saved before the check with an
unknown step fails at that step the same way.

A workspace macro run asks for one confirmation up front when the macro's own `sensitivity` is
`Restricted` or when any step needs one under the confirmation gate below, judged with the
run's initiator: a dangerous step, a `Restricted` one, or a `Mutating` one the agent or a plugin
started. Without a proof `executeMacro` runs no step and answers `CONFIRMATION_REQUIRED` with a
challenge for the macro, and its `message` names the reason: the sensitivity tier, the steps,
or both. The call made again with that proof runs the steps, and the one confirmation covers
every step of the run that needs one. A workspace macro with no such step runs without a
confirmation.

## Size Limits

Each call and each stored definition has a size limit, so one request costs a bounded amount of
work, storage and audit log. A request over a limit is refused with a `VALIDATION` error that
names it, such as "command arguments may be at most 65536 bytes".

| What | Limit |
|---|---|
| `executeCommand` `args`, serialized as JSON | 65,536 bytes, room for the longest message `chat.post_message` takes |
| `executeCommand` `commandId` | 256 bytes; a pipe expression (`isPipe`) 4,096 bytes |
| Macro (`createUserCommand`, `updateUserCommand`) | id 256 bytes, title 128 characters, description and `nlDescription` 1,000 characters each, up to 10 `intentSamples` of 200 characters, up to 20 steps, each step's id 256 bytes, `args` 4,096 bytes and up to 32 `ask` names of 64 bytes |
| Workspace macro (`createMacro`, `updateMacro`, deprecated) | description and `nlDescription` 1,000 characters each, up to 10 `intentSamples` of 200 characters, each step's id 256 bytes and `args` 4,096 bytes |
| Pipeline (`createPipeline`, `updatePipeline`) | description 1,000 characters, up to 5 filter conditions, a `MessageContains` pattern of 256 characters, up to 10 actions, each action's id 256 bytes and `args` 4,096 bytes |

Too large calls are refused before they run, so they write no audit row. The audit log keeps a
call's redacted arguments when they serialize to 4,096 bytes or less; larger arguments are kept
as `{"truncated": true, "bytes": <size>, "prefix": <the first 1,024 bytes of the redacted
JSON>}`.

What a person wrote is left out of the redacted arguments, with only its length kept. A string
under the name `text`, `query`, `reason`, `description`, `title` or `value`, at any depth and in
any command, is kept as `{"redacted_content": true, "chars": <count>}`. A plugin command's other
arguments are not known to the server by name, so they stay as they are, redacted only where their
name or value looks secret.

The same rules apply to a pipe expression, which the row keeps as its `commandId`: each stage's id
stays, with its `key=value` pairs masked, so `chat.post_message text=hello` is kept as
`chat.post_message text=[REDACTED:5]`, and a secret as `key=[REDACTED]`. An expression that does
not parse is kept as `unparsed | pipe` with the error kind `InvalidPipeExpression`, while the caller
still gets the message saying what is wrong with it.

An `AGENT` call may carry a `runId`, an identifier the desktop app gives one stretch of an
agent's calls. The audit entry keeps it, so `listAuditLog` can list that stretch; on a call with
any other initiator it is ignored. It is the client's word, like the initiator. It must be 1 to
64 ASCII letters, digits, `_` or `-`, or the call is refused with `VALIDATION` whatever the
initiator. A macro the agent runs is one call and carries it there; the steps of a macro the
client runs are `MACRO` calls and do not.

A pipeline action may not name a macro or a workspace macro: each action runs as its own
command call with its own budget of 100 builtin steps, and a pipeline fires on events other
people cause. `createPipeline` and `updatePipeline` refuse such an action with "pipeline actions must not
reference user commands or macros". An action must name a builtin command: `createPipeline` and
`updatePipeline` refuse any other id, a plugin command included, with `VALIDATION` "Unknown
command: <id>". A pipeline saved before these limits runs at most its first 10 actions and skips
an action naming a macro or a workspace macro. A Post as bot action is the one that runs without a
confirmation, as a bot rather than as the pipeline's creator (`bots.md`).

The server keeps at most 32 unanswered confirmation challenges per user; issuing one more drops
that user's challenge closest to expiry, which then answers `CONFIRMATION_STALE`. A pipeline's
action that needs a confirmation issues a challenge nobody answers, and this keeps such
challenges from piling up.

## Command Contract

Each command should expose the following metadata.

### Required Fields

- `id`
- `title`
- `description`
- `source_kind`
- `execution_target`
- `arguments`
- `required_context`
- `required_capabilities`

### Recommended Fields

- `visible_in_palette`
- `visible_in_context_menus`
- `visible_in_keybind_picker`
- `dangerous`
- `supports_preview`
- `supports_dry_run`

### Confirmation gate (H1)

A call needs a confirmation when the command is dangerous or `Restricted`, or when it is
`Mutating` and the agent or a plugin started it. Without a proof, `executeCommand` answers with
status `CONFIRMATION_REQUIRED` and the challenge `confirmationRequired { token, argsHash,
expiresAt }`. The client asks the person to confirm and calls again with
`confirmation: { inBandToken: { token, argsHash } }` before `expiresAt`, which is 60 seconds
after the challenge was issued. The proof is good once, for the same caller, command, arguments
and context; one that expired or was used answers `CONFIRMATION_STALE`, and one made for
something else `CONFIRMATION_FAILED`.

A `BOT` call comes from the bot API (`bots.md`), the one initiator the server sets itself: GraphQL has
no value for it, so a client cannot claim it, and the audit log shows it as `bot`. It is not agentic,
so a `Mutating` command such as `chat.post_message` needs no proof. Nobody is there to confirm, so a
command that would answer `CONFIRMATION_REQUIRED` is refused as `DENIED` on that API, and the commands
it runs need none.

### Timeout gate (T1)

After the capability and confirmation gates, `execute_command` refuses any command whose `sensitivity` is `Mutating` or
`Restricted` with `ServiceError::TimedOut { until }` (GraphQL `TIMED_OUT`, extension `until`)
when the actor is timed out in the context's workspace (`ModerationService::active_timeout`).
`Safe` and `Sensitive` commands still run. The gate also runs on nested dispatch (macro
steps, pipe stages), before the rate limiter, inside the audit wrapper. Without a workspace
context it has nothing to check; the services behind the command apply their own write gates
anyway. Gate order: C1 capability → H1 confirmation → T1 timeout → R1 rate limit.

### Refusals

An error from `executeCommand` or `executeMacro` carries a `refusal` extension next to its `code`,
which says what the caller can do about it:

| `refusal` | Meaning | Other extensions |
|---|---|---|
| `DENIED` | A policy decision: repeating the call does not help. | `missing`, the capability the caller lacks, when the capability gate refused; `until` when timed out; `feature` when the server has the feature off |
| `FAILED` | A technical problem, not a decision. | `retrySafe`; `retryAfterSeconds` when rate-limited |
| `EXPIRED` | The confirmation expired or was already used. Nothing ran. | none |
| `INVALID` | The call itself is wrong: bad arguments, nothing by that name, or a conflict. | none |

A command that needs a confirmation is not refused: it answers status `CONFIRMATION_REQUIRED`.

`retrySafe` is true only when repeating the call cannot change anything twice: every command it
reaches is `Safe` or `Sensitive` and none is dangerous, or the rate limiter stopped it before it
started. A `Mutating` command that timed out may have run, so it is false even if nothing changed. A
macro that fails outside its steps is false as well.

`missing` names a capability, never a workspace, channel or person, so it reveals nothing the caller
cannot already see: a capability id is public on the command's definition. At the capability gate,
someone who is not a member gets the same answer as someone whose role is too low, whether or not
the workspace exists. A client that names the target takes it from what it already shows.

A pipe that fails partway answers status `ERROR` with the failed stage's `message`, with no
`refusal`. A macro's failed step is reported the same way. A server older than this has no
`refusal`, so a client falls back on `code`.

## Arguments

Arguments should be typed from day one.

Initial recommended argument kinds:

- `string`
- `integer`
- `float`
- `boolean`
- `enum`
- `user_id`
- `conversation_id`
- `message_id`
- `thread_id`
- `workspace_id`
- `selection_text`

Each argument definition should include:

- `name`
- `title`
- `description`
- `type`
- `required`
- `default_value` if applicable
- `allowed_values` for enums

Arguments should be usable by:

- command palette forms
- plugin-provided UIs
- the steps of macros, fixed or asked for when the macro runs
- API surfaces later

## Context Model For Commands

Commands should explicitly declare required context.

Examples of context sources:

- selected text
- selected message
- selected thread
- current conversation
- current workspace
- active call/session
- current user/principal

This is important for:

- discoverability
- context menu filtering
- keybind eligibility
- plugin action registration

If required context is missing, the command must not be silently invoked.

## Invocation Sources

Commands should be invokable from multiple surfaces.

### Command Palette

- Main discovery surface.
- Should show only commands valid for the current context unless the user explicitly asks for all.

### Keybinds

- Keybinds should point to commands, not to custom one-off handlers.
- A keybind may provide fixed arguments or derive arguments from current context.

### Context Actions

- Text selection, message menus, thread actions, channel actions, etc.
- A context action should usually map to a command invocation.

### Plugin UI

- Buttons, panels, modals, or toolbars can invoke commands: the plugin's own commands and the
  ids it was granted in `runs_commands` (Who Can Run Commands).

### Automations

- Event-driven or scheduled workflows may invoke commands later.

## Who Can Run Commands

Every caller runs the same commands. What differs is which commands each one is offered and who
confirms a call. The client enforces the rules for agents and plugins; the server sees an
ordinary call with its initiator and applies its gates (Confirmation gate, Timeout gate, below)
as for any other.

**The person** runs every command, from every surface: the palette, keybinds, Home quick boxes,
the message box and menus. The client sends each call to the server that hosts the workspace it
runs in.

**A macro** runs any command as a step: built-in, app and plugin commands, each in turn,
started by the person (Commands And Macros, above). The person's own macros are the only ones
anyone runs.

**The in-app agent** runs `ai_callable` built-ins under the confirmation gate, and the person's
agent-callable macros on their home server (Running a macro, above). App commands are never
offered to an agent: a command only a person runs is an app command.

**An outside agent** is one connected through the desktop app's agent bridge, such as an MCP
client:

- It is offered the `ai_callable` built-ins that the server executes and whose sensitivity is
  not `Safe`. It is never offered a `DesktopLocal` built-in, an agent-management command (the
  `agents.*` app commands) or `chat.post_message`: it posts with its send tool, which handles
  encrypted conversations. It reads only through its read tools, which keep to the conversations
  the person chose. It is also offered the person's agent-callable macros (Running a macro,
  above) whose sensitivity is not `Safe`, under the same rules: each approved per agent and
  asked for on every call.
- It runs a command only after the person approved that command for that agent, in the same
  dialog as plugin commands. An approval is per agent and per command id.
- Every call shows the outside-agent card and runs only if the person approves it. A `dangerous`
  command is only proposed, never run.
- Its context comes only from ids in the arguments, never from what the person has open, and
  each id must be inside the agent's grants.
- The call runs with initiator `AGENT`. The card is the person's confirmation: a command the
  confirmation gate stops is confirmed with the person's answer to the card, and they are not
  asked twice.

**A plugin** runs its own commands (Plugin-Provided Commands, above), and the command ids its
manifest lists in `runs_commands` that the person granted at install or update:

- A built-in that is neither `dangerous` nor `Restricted` runs with initiator `PLUGIN`. A
  `Mutating` one asks the person on every call, in a confirmation that names the plugin: the
  server answers `CONFIRMATION_REQUIRED`, and the client confirms only when the person does.
- A `Safe` `DesktopLocal` built-in, and an app command the client marks as runnable by plugins,
  runs at once. Only app commands that open or show something, move focus or the selection, or
  start a form the person finishes are marked.
- A plugin never runs another plugin's commands, an agent-management command, a macro
  (`user.*`: a macro is the person's own), or a `dangerous` or `Restricted` command, whatever
  its manifest lists.
- A built-in's context is the person's current workspace, conversation and thread, never a
  selected message; `server_id`, `conversation_id` and `thread_id` in the arguments take
  precedence over it.
- The plugin receives the status, an error code and the message, never the command's output
  data.
- A plugin makes at most 20 command calls a minute. After the person dismisses a confirmation,
  that plugin's command calls are refused for 60 seconds. Every run is recorded in the plugin's
  audit.
- When an update adds ids to `runs_commands`, the added ones are not granted until the person
  approves them.

The manifest field and the plugin's call are in `specs/plugin-protocol-v1.md`, sections 12
and 9, and work the same for web plugins (`specs/web-plugins-v1.md`).

## Execution Targets

Every command should declare where it runs.

Initial execution targets:

- `backend`
- `desktop_local`
- `plugin_runtime`
- `integration_runtime`

Why this matters:

- security and capabilities differ per target
- availability differs by platform
- result handling and latency expectations differ

## Command Results

Every command should return a structured result envelope.

Decision:

- commands should not be modeled as only success/failure
- even simple commands should return a consistent result shape

Minimum result envelope should include:

- `status`
- `message` or summary
- optional structured payload
- optional follow-up actions

This supports:

- command palette feedback
- automation chaining
- plugin UI responses
- audit/event recording later

## Dangerous Local Commands

User-defined commands may run local OS processes on desktop.

Decision:

- allowed, but only with an explicit dangerous capability

Additional rule:

- local process execution should show a confirmation dialog by default
- the user may disable the confirmation later
- the safe default is confirmation enabled

This applies especially to commands such as:

- running shell commands
- launching local tools
- opening paths or files
- interacting with local system automation

The confirmation UX should clearly show:

- what command will run
- what runtime/capability is involved
- whether the command came from:
  - built-in logic
  - a plugin
  - an integration
  - a user-defined command

## Security Rules

Commands must inherit the security model of their provider.

Examples:

- built-in commands use built-in permissions
- plugin commands use plugin-declared capabilities
- integration commands use integration capabilities and tokens
- user-defined commands cannot exceed the capabilities of the commands they compose plus any local
  execution permissions explicitly granted by the user

Security rules:

- no implicit privilege escalation
- no hidden local process execution
- no hidden remote side effects
- dangerous commands must be marked clearly

## Availability Rules

A command may be:

- globally available
- available only in certain contexts
- available only on certain platforms
- available only when a required capability is granted
- available only when the workspace has the command's optional module (`feature`) turned on

Visibility: the `commands` and `commandDiscover` queries list only the commands the caller could
run in the given context, so the palette and the AI agent never offer one that `executeCommand`
refuses for who the caller is. A command the server runs (`backend`) is listed only when the
context names a workspace the caller is a member of, their role there may run it, and each of its
required capabilities passes the capability gate; a macro needs the role every step
needs. The role a builtin needs is the one the service behind it enforces: the Owner for
`admin.invite_member`; an Owner or Moderator for `admin.kick`, `admin.ban`, `admin.unban`,
`admin.timeout`, `admin.set_role`, `admin.delete_member_messages`, `chat.set_slow_mode`,
`chat.set_channel_posting`,
`chat.archive_channel` and `chat.unarchive_channel`; a member (not a guest) for `chat.create_channel`, `chat.create_group`
and `chat.invite_to_conversation`; a member, or a guest in the direct space, for
`chat.join_channel`; any member, guests included, for the rest. In the direct space, where
everyone is a guest, a group is started and people are invited through `createConversation` and
`addConversationMembers`, which apply the DM-request checks (the daily cap on requests to
strangers and the probation of new accounts) that the commands do not. The commands the client
runs itself (`desktop_local`) need no workspace and are listed to every signed-in caller, with a
workspace or without. Naming the direct space makes the caller a guest of it, as other reads
there do. A listed command can still be refused for its arguments, a confirmation it needs, or a
timeout in force.

Examples:

- `chat.post_message` may require conversation context or explicit arguments
- `plugin.k8s.restart_pod` may require desktop runtime and plugin capability approval
- `integration.youtrack.create_issue` may require integration installation and auth

## Example Command Shapes

### Built-in Command

- `id`: `chat.post_message`
- target: `backend`
- context: current conversation optional
- capabilities: `chat.write`

### Integration Command

- `id`: `integration.youtrack.create_issue`
- target: `integration_runtime`
- context: selected text or selected message optional
- capabilities: `action.invoke`, integration-specific external access

### Plugin Command

- `id`: `plugin.k8s.restart_pod`
- target: `plugin_runtime`
- context: selected pod or active panel context
- capabilities: plugin-declared runtime permissions

### User-Defined Command

- `id`: `user.clone_repo_and_open`
- target: `desktop_local`
- composed from:
  - `workspace.prompt_input`
  - local process execution
  - `workspace.open_folder`
- dangerous capability required because it runs a local process

## Non-Goals

The command model does not define:

- a scripting language: a pipe passes one command's result to the next stage, a macro runs its
  steps in order with arguments fixed or asked for, and anything beyond that belongs in a plugin
- dataflow beyond a linear pipe
- distributed workflow execution
- a typed payload schema per command result: results share one envelope
- the UI for editing commands, which is the client's concern

## Summary Of Decisions

- Commands are the common execution unit across palette, keybinds, context actions, plugins, and
  automations.
- Command ids are namespaced, and a released built-in id never changes; new built-ins use the
  namespace of their feature.
- Every built-in is in one of sixteen fixed categories.
- App commands run in the client only; the server knows their ids for keybinds and nothing else.
- Approving or refusing an outside agent is an app command no agent, plugin or pipe can run, and
  it always asks the person with a warning.
- The person runs every command. An outside agent runs only the built-ins the person approved
  for it, asking each time; a plugin runs only its own commands and the ids the person granted
  in its `runs_commands` (Who Can Run Commands).
- A command is one action RIM or a plugin gives you; a macro is the person's own action made of
  commands, and every macro is a command, in category `Macro`, with an id `user.<key>`.
- A macro's steps may be built-in, app or plugin commands, each argument fixed or asked for when
  it runs. The client runs a macro step by step and the first failing step stops the rest.
- Macros and keybinds live with the account on its home server; command calls go to the server
  that hosts the workspace.
- The AI agent runs a macro only when the person allowed it and every step is a built-in the
  server runs and the agent may run itself, with every argument fixed; it runs as one call on
  the home server.
- Workspace macros (`createMacro` and the rest) are deprecated; the client moves them into the
  person's macros once.
- Commands use typed arguments.
- Commands declare required context.
- Commands declare execution target.
- Commands return structured result envelopes.
- User-defined local process execution is allowed on desktop only behind an explicit dangerous
  capability.
- Dangerous local commands should show a confirmation dialog by default, with an option to disable
  it later.

### Default-workspace unbans

`admin.unban` restores a local account to the current default workspace as Member with public
channels, atomically with lifting the ban. The audit action records
`restored_default_workspace`; reasons remain flags rather than free text. Other workspaces
still need a fresh invite after unban.
