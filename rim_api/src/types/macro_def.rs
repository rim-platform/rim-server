use async_graphql::SimpleObject;

use super::CommandSensitivity;

// ── Macro ────────────────────────────────────────────────────────────────

#[derive(SimpleObject, Clone, Debug)]
pub struct MacroStep {
    pub command_id: String,
    pub args: Option<async_graphql::Json<serde_json::Value>>,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MacroDef {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub user_id: async_graphql::ID,
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<MacroStep>,
    pub ai_callable: bool,
    pub nl_description: String,
    pub intent_samples: Vec<String>,
    pub sensitivity: CommandSensitivity,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::macro_def::dto::MacroDto> for MacroDef {
    fn from(value: rim_services::macro_def::dto::MacroDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            user_id: async_graphql::ID(value.user_id.to_string()),
            name: value.name,
            description: value.description,
            steps: value
                .steps
                .into_iter()
                .map(|s| MacroStep {
                    command_id: s.command_id,
                    args: s.args.map(async_graphql::Json),
                })
                .collect(),
            ai_callable: value.ai_callable,
            nl_description: value.nl_description,
            intent_samples: value.intent_samples,
            sensitivity: CommandSensitivity::from(value.sensitivity),
            created_at: value.created_at,
        }
    }
}
