use async_graphql::{Enum, InputObject, SimpleObject};

// ── Pipeline GraphQL types ──────────────────────────────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PipelineTriggerKindGql {
    NewMessage,
    MessageUpdated,
    /// A message was deleted one at a time; a bulk delete (`deleteMemberMessages`) fires none.
    MessageDeleted,
    ReactionsUpdated,
    MemberJoined,
    WorkspaceStateChanged,
}

impl From<rim_services::pipeline::dto::PipelineTriggerKindDto> for PipelineTriggerKindGql {
    fn from(value: rim_services::pipeline::dto::PipelineTriggerKindDto) -> Self {
        use rim_services::pipeline::dto::PipelineTriggerKindDto as D;
        match value {
            D::NewMessage => Self::NewMessage,
            D::MessageUpdated => Self::MessageUpdated,
            D::MessageDeleted => Self::MessageDeleted,
            D::ReactionsUpdated => Self::ReactionsUpdated,
            D::MemberJoined => Self::MemberJoined,
            D::WorkspaceStateChanged => Self::WorkspaceStateChanged,
        }
    }
}

impl From<PipelineTriggerKindGql> for rim_services::pipeline::dto::PipelineTriggerKindDto {
    fn from(value: PipelineTriggerKindGql) -> Self {
        match value {
            PipelineTriggerKindGql::NewMessage => Self::NewMessage,
            PipelineTriggerKindGql::MessageUpdated => Self::MessageUpdated,
            PipelineTriggerKindGql::MessageDeleted => Self::MessageDeleted,
            PipelineTriggerKindGql::ReactionsUpdated => Self::ReactionsUpdated,
            PipelineTriggerKindGql::MemberJoined => Self::MemberJoined,
            PipelineTriggerKindGql::WorkspaceStateChanged => Self::WorkspaceStateChanged,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PipelineTriggerGql {
    pub kind: PipelineTriggerKindGql,
    pub conversation_id: Option<async_graphql::ID>,
}

impl From<rim_services::pipeline::dto::PipelineTriggerDto> for PipelineTriggerGql {
    fn from(value: rim_services::pipeline::dto::PipelineTriggerDto) -> Self {
        Self {
            kind: value.kind.into(),
            conversation_id: value
                .conversation_id
                .map(|id| async_graphql::ID(id.to_string())),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PipelineFilterConditionGql {
    pub condition_type: String,
    pub pattern: Option<String>,
    pub user_id: Option<async_graphql::ID>,
    pub conversation_id: Option<async_graphql::ID>,
}

impl From<rim_services::pipeline::dto::FilterConditionDto> for PipelineFilterConditionGql {
    fn from(value: rim_services::pipeline::dto::FilterConditionDto) -> Self {
        use rim_services::pipeline::dto::FilterConditionDto as D;
        match value {
            D::MessageContains { pattern } => Self {
                condition_type: "MessageContains".to_string(),
                pattern: Some(pattern),
                user_id: None,
                conversation_id: None,
            },
            D::MessageFromUser { user_id } => Self {
                condition_type: "MessageFromUser".to_string(),
                pattern: None,
                user_id: Some(async_graphql::ID(user_id.to_string())),
                conversation_id: None,
            },
            D::ConversationIs { conversation_id } => Self {
                condition_type: "ConversationIs".to_string(),
                pattern: None,
                user_id: None,
                conversation_id: Some(async_graphql::ID(conversation_id.to_string())),
            },
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PipelineFilterGql {
    pub conditions: Vec<PipelineFilterConditionGql>,
}

impl From<rim_services::pipeline::dto::PipelineFilterDto> for PipelineFilterGql {
    fn from(value: rim_services::pipeline::dto::PipelineFilterDto) -> Self {
        Self {
            conditions: value.conditions.into_iter().map(Into::into).collect(),
        }
    }
}

/// A message a bot posts when someone joins.
#[derive(SimpleObject, Clone, Debug)]
pub struct PipelinePostAsBotGql {
    pub bot_user_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    /// The text as saved, with its placeholders.
    pub text: String,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PipelineActionGql {
    pub action_type: String,
    /// For a Post as bot action, `chat.post_message`, the command it runs.
    pub command_id: Option<String>,
    /// For a Post as bot action, the bot, channel and text, as a client that does not know
    /// `postAsBot` shows them.
    pub args: Option<async_graphql::Json<serde_json::Value>>,
    /// The bot, channel and text of a Post as bot action; null for any other.
    pub post_as_bot: Option<PipelinePostAsBotGql>,
}

impl From<rim_services::pipeline::dto::PipelineActionDto> for PipelineActionGql {
    fn from(value: rim_services::pipeline::dto::PipelineActionDto) -> Self {
        use rim_services::pipeline::dto::PipelineActionDto as D;
        match value {
            D::ExecuteCommand { command_id, args } => Self {
                action_type: "ExecuteCommand".to_string(),
                command_id: Some(command_id),
                args: args.map(async_graphql::Json),
                post_as_bot: None,
            },
            D::PostAsBot {
                bot_user_id,
                conversation_id,
                text,
            } => Self {
                action_type: "PostAsBot".to_string(),
                command_id: Some(rim_services::pipeline::dto::POST_AS_BOT_COMMAND.to_string()),
                args: Some(async_graphql::Json(
                    rim_services::pipeline::dto::post_as_bot_args(
                        bot_user_id,
                        conversation_id,
                        &text,
                    ),
                )),
                post_as_bot: Some(PipelinePostAsBotGql {
                    bot_user_id: async_graphql::ID(bot_user_id.to_string()),
                    conversation_id: async_graphql::ID(conversation_id.to_string()),
                    text,
                }),
            },
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PipelineGql {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub name: String,
    pub description: Option<String>,
    pub trigger: PipelineTriggerGql,
    pub filter: Option<PipelineFilterGql>,
    pub actions: Vec<PipelineActionGql>,
    pub enabled: bool,
    pub cooldown_secs: Option<i32>,
    pub created_by: async_graphql::ID,
    pub schema_version: i32,
    pub last_fired_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::pipeline::dto::PipelineDto> for PipelineGql {
    fn from(value: rim_services::pipeline::dto::PipelineDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            name: value.name,
            description: value.description,
            trigger: value.trigger.into(),
            filter: value.filter.map(Into::into),
            actions: value.actions.into_iter().map(Into::into).collect(),
            enabled: value.enabled,
            cooldown_secs: value.cooldown_secs,
            created_by: async_graphql::ID(value.created_by.to_string()),
            schema_version: value.schema_version,
            last_fired_at: value.last_fired_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

// Input types for pipeline mutations

#[derive(InputObject, Clone, Debug)]
pub struct PipelineTriggerInput {
    pub kind: PipelineTriggerKindGql,
    pub conversation_id: Option<async_graphql::ID>,
}

#[derive(InputObject, Clone, Debug)]
pub struct PipelineFilterConditionInput {
    pub condition_type: String,
    pub pattern: Option<String>,
    pub user_id: Option<async_graphql::ID>,
    pub conversation_id: Option<async_graphql::ID>,
}

#[derive(InputObject, Clone, Debug)]
pub struct PipelineFilterInput {
    pub conditions: Vec<PipelineFilterConditionInput>,
}

/// A message a bot posts when someone joins.
#[derive(InputObject, Clone, Debug)]
pub struct PipelinePostAsBotInput {
    /// An active bot of the workspace.
    pub bot_user_id: async_graphql::ID,
    /// A plaintext channel that is not archived, which the bot is already in.
    pub conversation_id: async_graphql::ID,
    /// What it posts, which may hold `{username}` and `{name}`.
    pub text: String,
}

#[derive(InputObject, Clone, Debug)]
pub struct PipelineActionInput {
    /// The command to run; `chat.post_message` with `postAsBot`.
    pub command_id: String,
    /// The command's arguments; left out with `postAsBot`.
    pub args: Option<async_graphql::Json<serde_json::Value>>,
    /// Makes this a Post as bot action, on the Member Joined trigger only. Owner or Moderator.
    pub post_as_bot: Option<PipelinePostAsBotInput>,
}

#[derive(InputObject, Clone, Debug)]
pub struct CreatePipelineInput {
    pub server_id: async_graphql::ID,
    pub name: String,
    pub description: Option<String>,
    pub trigger: PipelineTriggerInput,
    pub filter: Option<PipelineFilterInput>,
    pub actions: Vec<PipelineActionInput>,
    pub cooldown_secs: Option<i32>,
}

#[derive(InputObject, Clone, Debug)]
pub struct UpdatePipelineInput {
    pub pipeline_id: async_graphql::ID,
    pub name: String,
    pub description: Option<String>,
    pub trigger: PipelineTriggerInput,
    pub filter: Option<PipelineFilterInput>,
    pub actions: Vec<PipelineActionInput>,
    pub cooldown_secs: Option<i32>,
}
