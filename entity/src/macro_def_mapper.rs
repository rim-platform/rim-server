use crate::macro_def;
use rim_core::macro_def::model::MacroDef;

impl From<macro_def::Model> for MacroDef {
    fn from(m: macro_def::Model) -> Self {
        Self {
            id: m.id.into(),
            server_id: m.server_id.into(),
            user_id: m.user_id.into(),
            name: m.name,
            description: m.description,
            steps_json: m.steps,
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
        }
    }
}
