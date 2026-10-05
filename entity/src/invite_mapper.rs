use crate::invite;
use rim_core::auth::model::Invite;

impl From<invite::Model> for Invite {
    fn from(m: invite::Model) -> Self {
        Self {
            id: m.id.into(),
            server_id: m.server_id.into(),
            created_by_user_id: m.created_by_user_id.into(),
            token_hash: m.token_hash,
            created_at: m.created_at.into(),
            expires_at: m.expires_at.map(Into::into),
            max_uses: m.max_uses,
            use_count: m.use_count,
            revoked_at: m.revoked_at.map(Into::into),
            role: m.role.into(),
        }
    }
}
