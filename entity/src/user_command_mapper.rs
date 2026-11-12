use crate::user_command;
use rim_core::command::model::UserCommand;

impl From<user_command::Model> for UserCommand {
    fn from(m: user_command::Model) -> Self {
        let intent_samples: Vec<String> = m
            .intent_samples
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            id: m.id.into(),
            owner_user_id: m.owner_user_id.into(),
            command_key: m.command_key,
            title: m.title,
            description: m.description,
            steps_json: m.steps_json,
            is_enabled: m.is_enabled,
            ai_callable: m.ai_callable,
            nl_description: m.nl_description,
            intent_samples,
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
        }
    }
}
