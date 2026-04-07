use crate::channel_lens;
use rim_core::channel_lens::model::ChannelLens;

impl From<channel_lens::Model> for ChannelLens {
    fn from(m: channel_lens::Model) -> Self {
        let filter_spec: rim_core::channel_lens::model::FilterSpec =
            serde_json::from_value(m.filter_spec).unwrap_or_default();

        Self {
            id: m.id.into(),
            server_id: m.server_id.into(),
            conversation_id: m.conversation_id.into(),
            created_by: m.created_by.into(),
            name: m.name,
            filter_spec,
            is_shared: m.is_shared,
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
        }
    }
}
