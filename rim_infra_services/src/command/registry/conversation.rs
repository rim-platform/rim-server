use rim_services::command::dto::{
    CommandArgumentDto, CommandArgumentTypeDto, CommandDefinitionDto, CommandExecutionTargetDto,
    CommandSourceKindDto,
};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;

fn workspace_arg() -> CommandArgumentDto {
    CommandArgumentDto {
        name: "server_id".to_string(),
        title: "Workspace".to_string(),
        description: "Workspace/server identifier.".to_string(),
        argument_type: CommandArgumentTypeDto::WorkspaceId,
        required: true,
        default_value: None,
        allowed_values: None,
    }
}

/// Falls back to the focused conversation (`context.conversation_id`) when omitted.
fn conversation_arg(description: &str) -> CommandArgumentDto {
    CommandArgumentDto {
        name: "conversation_id".to_string(),
        title: "Conversation".to_string(),
        description: description.to_string(),
        argument_type: CommandArgumentTypeDto::ConversationId,
        required: true,
        default_value: None,
        allowed_values: None,
    }
}

/// The Track 1.1 membership/archive builtins: backend, mutating, palette and keybind visible.
/// Authorization (membership, Owner/Moderator role, private-channel visibility) is enforced by
/// `ConversationService`; the executors only adapt arguments.
fn membership_command(
    id: &str,
    title: &str,
    description: &str,
    arguments: Vec<CommandArgumentDto>,
) -> CommandDefinitionDto {
    CommandDefinitionDto {
        id: id.to_string(),
        title: title.to_string(),
        description: description.to_string(),
        category: "Chat".to_string(),
        source_kind: CommandSourceKindDto::BuiltIn,
        execution_target: CommandExecutionTargetDto::Backend,
        arguments,
        required_context: vec![],
        required_capabilities: vec!["chat.write".to_string()],
        visible_in_palette: true,
        visible_in_context_menus: false,
        keybind_capable: true,
        dangerous: false,
        user_command_id: None,
        nl_description: None,
        intent_samples: vec![],
        ai_callable: false,
        sensitivity: CommandSensitivityDto::Mutating,
        typed_verb: TypedVerb::Write,
        rate_limit_per_minute: None,
        feature: None,
    }
}

/// Opts `definition` into the agent surface (`ai_callable`) with its AI-facing metadata.
fn agent_callable(
    definition: CommandDefinitionDto,
    nl_description: &str,
    intent_samples: &[&str],
) -> CommandDefinitionDto {
    CommandDefinitionDto {
        nl_description: Some(nl_description.to_string()),
        intent_samples: intent_samples.iter().map(|s| s.to_string()).collect(),
        ai_callable: true,
        ..definition
    }
}

pub(super) fn commands() -> Vec<CommandDefinitionDto> {
    let mut commands = vec![CommandDefinitionDto {
        id: "chat.create_group".to_string(),
        title: "Create Group".to_string(),
        description: "Create a new group conversation.".to_string(),
        category: "Chat".to_string(),
        source_kind: CommandSourceKindDto::BuiltIn,
        execution_target: CommandExecutionTargetDto::Backend,
        arguments: vec![
            CommandArgumentDto {
                name: "server_id".to_string(),
                title: "Server".to_string(),
                description: "Workspace/server identifier.".to_string(),
                argument_type: CommandArgumentTypeDto::WorkspaceId,
                required: true,
                default_value: None,
                allowed_values: None,
            },
            CommandArgumentDto {
                name: "member_ids".to_string(),
                title: "Members".to_string(),
                description: "Member ids to include in the group.".to_string(),
                argument_type: CommandArgumentTypeDto::String,
                required: true,
                default_value: None,
                allowed_values: None,
            },
            CommandArgumentDto {
                name: "title".to_string(),
                title: "Title".to_string(),
                description: "Optional group title.".to_string(),
                argument_type: CommandArgumentTypeDto::String,
                required: false,
                default_value: None,
                allowed_values: None,
            },
        ],
        required_context: vec![],
        required_capabilities: vec!["chat.write".to_string()],
        visible_in_palette: true,
        visible_in_context_menus: false,
        keybind_capable: true,
        dangerous: false,
        user_command_id: None,
        nl_description: Some(
            "Create a group conversation with the given members. \
             Pass member_ids from chat.list_members; an optional title names the group."
                .to_string(),
        ),
        intent_samples: vec![
            "start a group with Anna and Petr".to_string(),
            "create a group chat for the release".to_string(),
        ],
        ai_callable: true,
        sensitivity: CommandSensitivityDto::Mutating,
        typed_verb: TypedVerb::Write,
        rate_limit_per_minute: None,
        feature: None,
    }];
    commands.extend(membership_commands());
    commands
}

fn membership_commands() -> Vec<CommandDefinitionDto> {
    vec![
        agent_callable(
            membership_command(
                "chat.join_channel",
                "Join Channel",
                "Join a public channel.",
                vec![workspace_arg(), conversation_arg("Public channel to join.")],
            ),
            "Join a public channel so it shows in the user's sidebar and they follow it. \
                 Use when the user asks to join or follow a channel. Private channels cannot \
                 be joined; they need an invitation from a member.",
            &[
                "join #design-reviews",
                "add me to the release channel",
                "I want to follow this channel",
            ],
        ),
        agent_callable(
            membership_command(
                "chat.leave_conversation",
                "Leave Conversation",
                "Leave a channel or group.",
                vec![
                    workspace_arg(),
                    conversation_arg("Channel or group to leave (not a direct message)."),
                ],
            ),
            "Leave a channel or group conversation; it disappears from the user's sidebar. \
                 Leaving a private channel removes access until someone invites the user back. \
                 Use when the user asks to leave, quit or stop following a conversation.",
            &[
                "leave this channel",
                "get me out of the old-project group",
                "stop following #random",
            ],
        ),
        agent_callable(
            membership_command(
                "chat.invite_to_conversation",
                "Invite to Conversation",
                "Add workspace members to a channel or group.",
                vec![
                    workspace_arg(),
                    conversation_arg("Channel or group to add the members to."),
                    CommandArgumentDto {
                        name: "user_ids".to_string(),
                        title: "Members".to_string(),
                        description: "User ids of the workspace members to add.".to_string(),
                        argument_type: CommandArgumentTypeDto::String,
                        required: true,
                        default_value: None,
                        allowed_values: None,
                    },
                ],
            ),
            "Add one or more workspace members to a channel or group the user belongs to. \
                 This is the only way into a private channel. Use when the user asks to invite \
                 or add someone to a conversation.",
            &[
                "invite alice to this channel",
                "add bob and carol to #incident-review",
                "bring the on-call engineer into this group",
            ],
        ),
        agent_callable(
            membership_command(
                "chat.archive_channel",
                "Archive Channel",
                "Archive a channel (Owner/Moderator). Archived channels are read-only.",
                vec![workspace_arg(), conversation_arg("Channel to archive.")],
            ),
            "Archive a channel: it becomes read-only and is hidden from the default channel \
                 list, but its history stays readable and searchable. Only workspace owners and \
                 moderators may do this. Use when the user asks to archive or close a channel.",
            &[
                "archive this channel",
                "close #migration-2025, the project is done",
                "make the old launch channel read-only",
            ],
        ),
        agent_callable(
            membership_command(
                "chat.unarchive_channel",
                "Unarchive Channel",
                "Unarchive a channel (Owner/Moderator).",
                vec![workspace_arg(), conversation_arg("Channel to unarchive.")],
            ),
            "Unarchive a channel so people can post in it again. Only workspace owners and \
                 moderators may do this. Use when the user asks to reopen or restore an \
                 archived channel.",
            &[
                "unarchive #launch",
                "reopen the archived design channel",
                "restore this channel",
            ],
        ),
        // Removing someone else is a moderation action: confirmation required, never agent-run.
        CommandDefinitionDto {
            dangerous: true,
            ..membership_command(
                "chat.remove_conversation_member",
                "Remove Member from Conversation",
                "Remove a member from a channel or group (Owner/Moderator, or yourself).",
                vec![
                    workspace_arg(),
                    conversation_arg("Channel or group to remove the member from."),
                    CommandArgumentDto {
                        name: "user_id".to_string(),
                        title: "Member".to_string(),
                        description: "User id of the member to remove.".to_string(),
                        argument_type: CommandArgumentTypeDto::UserId,
                        required: true,
                        default_value: None,
                        allowed_values: None,
                    },
                ],
            )
        },
    ]
}
