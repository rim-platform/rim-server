use crate::session;
use rim_core::auth::model::Session;

impl From<session::Model> for Session {
    fn from(m: session::Model) -> Self {
        Self {
            id: m.id.into(),
            user_id: m.user_id.into(),
            token_hash: m.token_hash,
            created_at: m.created_at.into(),
            expires_at: m.expires_at.into(),
            last_seen_at: m.last_seen_at.map(Into::into),
            revoked_at: m.revoked_at.map(Into::into),
            user_agent: m.user_agent,
            ip: m.ip,
            device_key_id: m.device_key_id.map(Into::into),
        }
    }
}
