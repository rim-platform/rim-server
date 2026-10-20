//! Accounts of a given age, for the probation tests of the services.

use chrono::{DateTime, Utc};
use rim_core::user::model::{User, UserId};
use rim_core::user::repository::MockUserRepository;

/// An account `id` created at `created_at`, holding `principal_id`.
pub(crate) fn account(id: UserId, created_at: DateTime<Utc>, principal_id: Option<&str>) -> User {
    User {
        id,
        username: format!("user-{id}"),
        password_hash: None,
        created_at,
        updated_at: created_at,
        last_login_at: None,
        display_name: None,
        avatar_url: None,
        status_text: None,
        bio: None,
        email: None,
        timezone: None,
        principal_id: principal_id.map(str::to_string),
        deletion_scheduled_for: None,
        handle: rim_core::user::model::UserHandle::Local,
    }
}

/// A user repository that finds each of `accounts` by id and nobody else.
pub(crate) fn users(accounts: Vec<User>) -> MockUserRepository {
    let mut repo = MockUserRepository::new();
    let by_id = accounts.clone();
    repo.expect_get_by_id()
        .returning(move |id| Ok(by_id.iter().find(|u| u.id == id).cloned()));
    repo.expect_get_by_ids().returning(move |ids| {
        Ok(accounts
            .iter()
            .filter(|u| ids.contains(&u.id))
            .cloned()
            .collect())
    });
    repo
}
