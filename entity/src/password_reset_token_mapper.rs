use crate::password_reset_token;
use rim_core::password_reset::model::PasswordResetToken;

impl From<password_reset_token::Model> for PasswordResetToken {
    fn from(m: password_reset_token::Model) -> Self {
        Self {
            id: m.id.into(),
            user_id: m.user_id.into(),
            token_hash: m.token_hash,
            expires_at: m.expires_at.into(),
            used_at: m.used_at.map(Into::into),
            created_at: m.created_at.into(),
        }
    }
}
