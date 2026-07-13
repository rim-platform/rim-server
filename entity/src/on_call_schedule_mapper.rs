use crate::on_call_schedule;
use rim_core::oncall::model::OnCallSchedule;

impl From<on_call_schedule::Model> for OnCallSchedule {
    fn from(m: on_call_schedule::Model) -> Self {
        Self {
            id: m.id.into(),
            server_id: m.server_id.into(),
            name: m.name,
            timezone: m.timezone,
            created_by: m.created_by.into(),
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
        }
    }
}
