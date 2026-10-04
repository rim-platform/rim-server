use crate::on_call_rotation_participant;
use rim_core::oncall::model::RotationParticipant;

impl From<on_call_rotation_participant::Model> for RotationParticipant {
    fn from(m: on_call_rotation_participant::Model) -> Self {
        Self {
            id: m.id.into(),
            layer_id: m.layer_id.into(),
            user_id: m.user_id.into(),
            position: m.position,
        }
    }
}
