use crate::instance_invite;
use rim_core::auth::model::InstanceInvite;

impl From<instance_invite::Model> for InstanceInvite {
    fn from(m: instance_invite::Model) -> Self {
        Self {
            id: m.id.into(),
            created_by_user_id: m.created_by_user_id.into(),
            token_hash: m.token_hash,
            created_at: m.created_at.into(),
            expires_at: m.expires_at.into(),
            max_uses: m.max_uses,
            use_count: m.use_count,
            revoked_at: m.revoked_at.map(Into::into),
        }
    }
}
