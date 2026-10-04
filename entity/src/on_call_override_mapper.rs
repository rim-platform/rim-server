use crate::on_call_override;
use rim_core::oncall::model::OnCallOverride;

impl From<on_call_override::Model> for OnCallOverride {
    fn from(m: on_call_override::Model) -> Self {
        Self {
            id: m.id.into(),
            schedule_id: m.schedule_id.into(),
            user_id: m.user_id.into(),
            start_at: m.start_at.into(),
            end_at: m.end_at.into(),
            created_by: m.created_by.into(),
        }
    }
}
