use crate::model_id;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};

model_id!(PasswordResetTokenId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordResetToken {
    pub id: PasswordResetTokenId,
    pub user_id: UserId,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPasswordResetToken {
    pub user_id: UserId,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
}
