use crate::user;
use rim_core::user::model::User;

impl From<user::Model> for User {
    fn from(m: user::Model) -> Self {
        Self {
            id: m.id.into(),
            username: m.username,
            password_hash: m.password_hash,
            created_at: m.created_at.into(),
            updated_at: m.updated_at.into(),
            last_login_at: m.last_login_at.map(Into::into),
        }
    }
}
