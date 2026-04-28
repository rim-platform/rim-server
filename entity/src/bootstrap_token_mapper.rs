use crate::bootstrap_token;
use rim_core::auth::model::BootstrapToken;

impl From<bootstrap_token::Model> for BootstrapToken {
    fn from(m: bootstrap_token::Model) -> Self {
        Self {
            id: m.id.into(),
            email: m.email,
            token_hash: m.token_hash,
            created_at: m.created_at.into(),
            expires_at: m.expires_at.into(),
            used_at: m.used_at.map(Into::into),
            consumed_by_user_id: m.consumed_by_user_id.map(Into::into),
            revoked_at: m.revoked_at.map(Into::into),
        }
    }
}
