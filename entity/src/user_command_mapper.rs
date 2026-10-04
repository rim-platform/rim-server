use crate::user_command;
use rim_core::command::model::UserCommand;

impl From<user_command::Model> for UserCommand {
    fn from(m: user_command::Model) -> Self {
        Self {
            id: m.id.into(),
            owner_user_id: m.owner_user_id.into(),
            command_key: m.command_key,
            title: m.title,
            description: m.description,
            steps_json: m.steps_json,
            is_enabled: m.is_enabled,
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
        }
    }
}
