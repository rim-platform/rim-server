use crate::pipeline;
use rim_core::pipeline::model::{Pipeline, PipelineAction, PipelineFilter, PipelineTrigger};

impl TryFrom<pipeline::Model> for Pipeline {
    type Error = serde_json::Error;

    fn try_from(m: pipeline::Model) -> Result<Self, Self::Error> {
        let trigger: PipelineTrigger = serde_json::from_value(m.trigger_data)?;
        let filter: Option<PipelineFilter> =
            m.filter_data.map(serde_json::from_value).transpose()?;
        let actions: Vec<PipelineAction> = serde_json::from_value(m.actions_data)?;

        Ok(Self {
            id: m.id.into(),
            server_id: m.server_id.into(),
            name: m.name,
            description: m.description,
            trigger,
            filter,
            actions,
            enabled: m.enabled,
            cooldown_secs: m.cooldown_secs,
            created_by: m.created_by.into(),
            schema_version: m.schema_version,
            last_fired_at: m.last_fired_at.map(|dt| dt.into()),
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
        })
    }
}
