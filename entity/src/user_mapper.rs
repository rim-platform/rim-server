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
            display_name: m.display_name,
            avatar_url: m.avatar_url,
            status_text: m.status_text,
            bio: m.bio,
            email: m.email,
            timezone: m.timezone,
            principal_id: m.principal_id,
            deletion_scheduled_for: m.deletion_scheduled_for.map(Into::into),
        }
    }
}
