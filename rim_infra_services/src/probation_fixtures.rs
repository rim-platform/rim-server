//! Accounts of a given age, for the probation tests of the services.

use chrono::{DateTime, Utc};
use rim_core::user::model::{User, UserId};
use rim_core::user::repository::MockUserRepository;

/// An account `id` created at `created_at`, holding `principal_id`.
pub(crate) fn account(id: UserId, created_at: DateTime<Utc>, principal_id: Option<&str>) -> User {
    User {
        id,
        username: format!("user-{id}"),
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
        profile_locked_until: None,
        handle: rim_core::user::model::UserHandle::Local,
    }
}

/// A user repository that finds each of `accounts` by id and nobody else, none of them an
/// instance operator.
pub(crate) fn users(accounts: Vec<User>) -> MockUserRepository {
    users_and_operators(accounts, Vec::new())
}

/// [`users`] where the accounts in `operators` hold the instance operator role.
pub(crate) fn users_and_operators(
    accounts: Vec<User>,
    operators: Vec<UserId>,
) -> MockUserRepository {
    let mut repo = MockUserRepository::new();
    repo.expect_is_instance_operator()
        .returning(move |id| Ok(operators.contains(&id)));
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
