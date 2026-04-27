use crate::macro_def;
use rim_core::macro_def::model::MacroDef;

impl From<macro_def::Model> for MacroDef {
    fn from(m: macro_def::Model) -> Self {
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
            server_id: m.server_id.into(),
            user_id: m.user_id.into(),
            name: m.name,
            description: m.description,
            steps_json: m.steps,
            ai_callable: m.ai_callable,
            nl_description: m.nl_description,
            intent_samples,
            sensitivity: m.sensitivity,
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
        }
    }
}
