use crate::server_member_timeout;
use rim_core::moderation::model::MemberTimeout;

impl From<server_member_timeout::Model> for MemberTimeout {
    fn from(m: server_member_timeout::Model) -> Self {
        Self {
            server_id: m.server_id.into(),
            user_id: m.user_id.into(),
            until: m.until.into(),
            reason: m.reason,
            set_by: m.set_by.map(Into::into),
            created_at: m.created_at.into(),
        }
    }
}
