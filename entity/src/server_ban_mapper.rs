use crate::server_ban;
use rim_core::moderation::model::ServerBan;

impl From<server_ban::Model> for ServerBan {
    fn from(m: server_ban::Model) -> Self {
        Self {
            server_id: m.server_id.into(),
            user_id: m.user_id.into(),
            reason: m.reason,
            banned_by: m.banned_by.map(Into::into),
            created_at: m.created_at.into(),
            expires_at: m.expires_at.map(Into::into),
        }
    }
}
