//! Moderation builtins. Authorization (Owner/Moderator role, who may target whom,
//! guest and timeout gates) lives in `ModerationService`; the executors only adapt arguments.

use rim_services::command::dto::{
    CommandArgumentDto, CommandArgumentTypeDto, CommandDefinitionDto, CommandExecutionTargetDto,
    CommandSourceKindDto,
};
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::verb::TypedVerb;

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

fn workspace_arg() -> CommandArgumentDto {
    arg(
        "server_id",
        "Workspace",
        "Workspace/server identifier.",
        CommandArgumentTypeDto::WorkspaceId,
        true,
    )
}

fn member_arg(description: &str) -> CommandArgumentDto {
    arg(
        "user_id",
        "Member",
        description,
        CommandArgumentTypeDto::UserId,
        true,
    )
}

fn reason_arg() -> CommandArgumentDto {
    arg(
        "reason",
        "Reason",
        "Optional reason, shown to moderators.",
        CommandArgumentTypeDto::String,
        false,
    )
}

/// A backend, Mutating builtin, palette and keybind visible, not agent-callable.
fn moderation_command(
    id: &str,
    title: &str,
    description: &str,
    category: &str,
    capability: &str,
    arguments: Vec<CommandArgumentDto>,
    typed_verb: TypedVerb,
) -> CommandDefinitionDto {
    CommandDefinitionDto {
        id: id.to_string(),
        title: title.to_string(),
        description: description.to_string(),
        category: category.to_string(),
        source_kind: CommandSourceKindDto::BuiltIn,
        execution_target: CommandExecutionTargetDto::Backend,
        arguments,
        required_context: vec![],
        required_capabilities: vec![capability.to_string()],
        visible_in_palette: true,
        visible_in_context_menus: false,
        keybind_capable: true,
        dangerous: false,
        user_command_id: None,
        nl_description: None,
        intent_samples: vec![],
        ai_callable: false,
        sensitivity: CommandSensitivityDto::Mutating,
        typed_verb,
        rate_limit_per_minute: None,
    }
}

fn admin_command(
    id: &str,
    title: &str,
    description: &str,
    arguments: Vec<CommandArgumentDto>,
    typed_verb: TypedVerb,
) -> CommandDefinitionDto {
    moderation_command(
        id,
        title,
        description,
        "Admin",
        "admin.manage_members",
        arguments,
        typed_verb,
    )
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
    vec![
        // Kick, ban and role changes are dangerous: confirmation required, never agent-run.
        CommandDefinitionDto {
            dangerous: true,
            ..admin_command(
                "admin.kick",
                "Kick Member",
                "Remove a member from the workspace (Owner/Moderator). They can re-join with a \
                 new invite.",
                vec![
                    workspace_arg(),
                    member_arg("User id of the member to kick."),
                    reason_arg(),
                ],
                TypedVerb::Delete,
            )
        },
        CommandDefinitionDto {
            dangerous: true,
            ..admin_command(
                "admin.ban",
                "Ban Member",
                "Remove a member and block them from re-joining (Owner/Moderator).",
                vec![
                    workspace_arg(),
                    member_arg("User id of the member to ban."),
                    reason_arg(),
                    arg(
                        "expires_at",
                        "Expires",
                        "Optional RFC 3339 time when the ban ends; omit to ban until unbanned.",
                        CommandArgumentTypeDto::String,
                        false,
                    ),
                ],
                TypedVerb::Delete,
            )
        },
        admin_command(
            "admin.unban",
            "Unban User",
            "Lift a workspace ban (Owner/Moderator).",
            vec![workspace_arg(), member_arg("User id of the banned user.")],
            TypedVerb::Write,
        ),
        agent_callable(
            admin_command(
                "admin.timeout",
                "Timeout Member",
                "Mute a member for a number of minutes (Owner/Moderator, at most 30 days).",
                vec![
                    workspace_arg(),
                    member_arg("User id of the member to time out."),
                    arg(
                        "minutes",
                        "Minutes",
                        "How long the timeout lasts, in minutes (1 to 43200).",
                        CommandArgumentTypeDto::Integer,
                        true,
                    ),
                    reason_arg(),
                ],
                TypedVerb::Write,
            ),
            "Time out a workspace member for a number of minutes: they can still read but \
             cannot send messages, react, pin or run commands that change anything until the \
             timeout ends. Only owners and moderators may do this, and moderators cannot time \
             out other moderators. Use when the user asks to mute or time out someone.",
            &[
                "time out @sam for 10 minutes",
                "mute this user for an hour",
                "give alex a one day timeout for spamming",
            ],
        ),
        CommandDefinitionDto {
            dangerous: true,
            ..admin_command(
                "admin.set_role",
                "Set Member Role",
                "Change a member's role (Owner: any role but owner; Moderator: member or guest).",
                vec![
                    workspace_arg(),
                    member_arg("User id of the member."),
                    CommandArgumentDto {
                        allowed_values: Some(vec![
                            "moderator".to_string(),
                            "member".to_string(),
                            "guest".to_string(),
                        ]),
                        ..arg(
                            "role",
                            "Role",
                            "New role: moderator, member or guest.",
                            CommandArgumentTypeDto::Enum,
                            true,
                        )
                    },
                ],
                TypedVerb::Configure,
            )
        },
        agent_callable(
            moderation_command(
                "chat.set_slow_mode",
                "Set Slow Mode",
                "Limit how often members may post in a channel (Owner/Moderator; 0 turns it \
                 off).",
                "Chat",
                "chat.write",
                vec![
                    workspace_arg(),
                    arg(
                        "conversation_id",
                        "Channel",
                        "Channel to configure; defaults to the focused conversation.",
                        CommandArgumentTypeDto::ConversationId,
                        true,
                    ),
                    arg(
                        "seconds",
                        "Seconds",
                        "Minimum seconds between two messages of the same member (0 to 21600).",
                        CommandArgumentTypeDto::Integer,
                        true,
                    ),
                ],
                TypedVerb::Configure,
            ),
            "Turn on slow mode in a channel: each member who is not a moderator must wait the \
             given number of seconds between two messages there. Zero turns slow mode off. \
             Only owners and moderators may do this. Use when the user asks to slow a channel \
             down or to rate-limit posting.",
            &[
                "turn on slow mode in this channel, 30 seconds",
                "slow #general down to one message a minute",
                "turn off slow mode",
            ],
        ),
        agent_callable(
            moderation_command(
                "chat.set_channel_posting",
                "Set Who Can Post",
                "Let everyone or only the owner and moderators post in a channel \
                 (Owner/Moderator).",
                "Chat",
                "chat.write",
                vec![
                    workspace_arg(),
                    arg(
                        "conversation_id",
                        "Channel",
                        "Channel to configure; defaults to the focused conversation.",
                        CommandArgumentTypeDto::ConversationId,
                        true,
                    ),
                    CommandArgumentDto {
                        allowed_values: Some(vec![
                            "everyone".to_string(),
                            "moderators".to_string(),
                        ]),
                        ..arg(
                            "policy",
                            "Who can post",
                            "everyone, or moderators for the owner and moderators only.",
                            CommandArgumentTypeDto::Enum,
                            true,
                        )
                    },
                    CommandArgumentDto {
                        default_value: Some(serde_json::Value::Bool(true)),
                        ..arg(
                            "allows_threads",
                            "Others can react and reply in threads",
                            "Under moderators, whether everyone else may still react and reply \
                             in threads. Defaults to true.",
                            CommandArgumentTypeDto::Boolean,
                            false,
                        )
                    },
                ],
                TypedVerb::Configure,
            ),
            "Set who may post in a channel: everyone, or only the workspace owner and \
             moderators. With moderators, allows_threads says whether everyone else may still \
             add reactions and reply in threads. Only owners and moderators may do this. Use \
             when the user asks to make a channel read-only or announcement-only, or to open it \
             to everyone again.",
            &[
                "make #announcements moderators only",
                "only moderators can post here, but let people reply in threads",
                "let everyone post in this channel again",
            ],
        ),
        agent_callable(
            CommandDefinitionDto {
                required_context: vec![
                    "workspace".to_string(),
                    "conversation".to_string(),
                    "selected_message".to_string(),
                ],
                visible_in_palette: false,
                visible_in_context_menus: true,
                ..moderation_command(
                    "chat.report_message",
                    "Report Message",
                    "Report the selected message to the workspace moderators.",
                    "Chat",
                    "chat.write",
                    vec![arg(
                        "reason",
                        "Reason",
                        "Why the message should be reviewed.",
                        CommandArgumentTypeDto::String,
                        true,
                    )],
                    TypedVerb::Write,
                )
            },
            "Report the selected message to the workspace moderators with a short reason. \
             Each member can report a message once. Use when the user asks to report or flag \
             a message for review.",
            &[
                "report this message as spam",
                "flag this for the moderators, it is harassment",
                "report the selected message",
            ],
        ),
    ]
}
