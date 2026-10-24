//! In-memory account passwords and the OPAQUE client steps, for the unit tests of the password
//! service and of the key sign-in.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use rim_auth::opaque::client::Registration;
use rim_auth::opaque::server_registration_start;
use rim_core::account_password::model::{
    AccountPassword, AttemptSchedule, AttemptStart, PasswordLogin, PasswordLoginId, PasswordReset,
    PasswordResetId,
};
use rim_core::account_password::repository::AccountPasswordRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::user::model::UserId;

use crate::password_key::PasswordKey;

/// One password key for every unit test.
pub(crate) static TEST_PASSWORD_KEY: LazyLock<PasswordKey> =
    LazyLock::new(|| PasswordKey::generate().expect("password key"));

/// A `RegistrationUpload` of `password` for `principal_id`, made against
/// [`TEST_PASSWORD_KEY`].
pub(crate) fn registration_upload(principal_id: &str, password: &[u8]) -> Vec<u8> {
    let (flow, request) = Registration::start(password);
    let response =
        server_registration_start(TEST_PASSWORD_KEY.setup(), &request, principal_id.as_bytes())
            .expect("registration response");
    flow.finish(&response).expect("registration upload").0
}

/// What [`FakePasswords`] stores.
#[derive(Default)]
pub(crate) struct PasswordState {
    pub(crate) passwords: HashMap<UserId, AccountPassword>,
    pub(crate) logins: HashMap<PasswordLoginId, PasswordLogin>,
    /// Newest last.
    pub(crate) rewraps: HashMap<UserId, Vec<Vec<u8>>>,
    pub(crate) resets: HashMap<PasswordResetId, PasswordReset>,
    /// By token hash: the account and the expiry.
    pub(crate) tickets: HashMap<String, (UserId, DateTime<Utc>)>,
}

/// Account passwords in memory, with the counting rules of the database implementation.
#[derive(Clone, Default)]
pub(crate) struct FakePasswords(pub(crate) Arc<Mutex<PasswordState>>);

impl FakePasswords {
    pub(crate) fn state(&self) -> std::sync::MutexGuard<'_, PasswordState> {
        self.0.lock().expect("password state")
    }
}

#[async_trait]
impl AccountPasswordRepository for FakePasswords {
    async fn get(&self, user_id: UserId) -> RepoResult<Option<AccountPassword>> {
        Ok(self.state().passwords.get(&user_id).cloned())
    }

    async fn insert(
        &self,
        user_id: UserId,
        registration: Vec<u8>,
        share_sealed: Vec<u8>,
        now: DateTime<Utc>,
    ) -> RepoResult<()> {
        let mut state = self.state();
        if state.passwords.contains_key(&user_id) {
            return Err(RepoError::new(RepoErrorKind::Conflict));
        }
        state.passwords.insert(
            user_id,
            AccountPassword {
                user_id,
                registration,
                share_sealed,
                failed_attempts: 0,
                next_attempt_at: None,
                locked_at: None,
                created_at: now,
                updated_at: now,
            },
        );
        Ok(())
    }

    async fn replace_registration(
        &self,
        user_id: UserId,
        registration: Vec<u8>,
        now: DateTime<Utc>,
    ) -> RepoResult<bool> {
        let mut state = self.state();
        let Some(password) = state.passwords.get_mut(&user_id) else {
            return Ok(false);
        };
        password.registration = registration;
        password.failed_attempts = 0;
        password.next_attempt_at = None;
        password.locked_at = None;
        password.updated_at = now;
        Ok(true)
    }

    async fn count_passwords(&self) -> RepoResult<u64> {
        Ok(self.state().passwords.len() as u64)
    }

    async fn start_attempt(
        &self,
        user_id: UserId,
        now: DateTime<Utc>,
        schedule: &AttemptSchedule,
    ) -> RepoResult<AttemptStart> {
        let mut state = self.state();
        let Some(password) = state.passwords.get_mut(&user_id) else {
            return Ok(AttemptStart::NoPassword);
        };
        if password.locked_at.is_some() {
            return Ok(AttemptStart::Locked);
        }
        if let Some(until) = password.next_attempt_at.filter(|at| *at > now) {
            return Ok(AttemptStart::Wait { until });
        }
        password.failed_attempts += 1;
        let count = password.failed_attempts;
        if schedule.locks_at(count) {
            password.locked_at = Some(now);
            password.next_attempt_at = None;
        } else {
            password.next_attempt_at = schedule.wait_after(count).map(|wait| now + wait);
        }
        password.updated_at = now;
        Ok(AttemptStart::Counted {
            registration: password.registration.clone(),
            failed_attempts: count,
        })
    }

    async fn clear_attempts(&self, user_id: UserId, now: DateTime<Utc>) -> RepoResult<()> {
        if let Some(password) = self.state().passwords.get_mut(&user_id) {
            password.failed_attempts = 0;
            password.next_attempt_at = None;
            password.locked_at = None;
            password.updated_at = now;
        }
        Ok(())
    }

    async fn insert_login(&self, login: PasswordLogin, now: DateTime<Utc>) -> RepoResult<()> {
        let mut state = self.state();
        state.logins.retain(|_, l| l.expires_at > now);
        state.logins.insert(login.id, login);
        Ok(())
    }

    async fn take_login(&self, id: PasswordLoginId) -> RepoResult<Option<PasswordLogin>> {
        Ok(self.state().logins.remove(&id))
    }

    async fn insert_ticket(
        &self,
        token_hash: &str,
        user_id: UserId,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> RepoResult<()> {
        let mut state = self.state();
        state.tickets.retain(|_, (_, at)| *at > now);
        state
            .tickets
            .insert(token_hash.to_string(), (user_id, expires_at));
        Ok(())
    }

    async fn consume_ticket(
        &self,
        token_hash: &str,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<bool> {
        let mut state = self.state();
        let valid = state
            .tickets
            .get(token_hash)
            .is_some_and(|(owner, at)| *owner == user_id && *at > now);
        if valid {
            state.tickets.remove(token_hash);
        }
        Ok(valid)
    }

    async fn insert_rewrap(
        &self,
        user_id: UserId,
        rewrap: Vec<u8>,
        _now: DateTime<Utc>,
        keep: u32,
    ) -> RepoResult<()> {
        let mut state = self.state();
        let rewraps = state.rewraps.entry(user_id).or_default();
        rewraps.push(rewrap);
        let keep = keep as usize;
        if rewraps.len() > keep {
            let excess = rewraps.len() - keep;
            rewraps.drain(..excess);
        }
        Ok(())
    }

    async fn list_rewraps(&self, user_id: UserId) -> RepoResult<Vec<Vec<u8>>> {
        Ok(self
            .state()
            .rewraps
            .get(&user_id)
            .map(|r| r.iter().rev().cloned().collect())
            .unwrap_or_default())
    }

    async fn delete_rewraps(&self, user_id: UserId) -> RepoResult<u64> {
        Ok(self
            .state()
            .rewraps
            .remove(&user_id)
            .map_or(0, |r| r.len() as u64))
    }

    async fn insert_reset(&self, reset: PasswordReset, now: DateTime<Utc>) -> RepoResult<()> {
        let mut state = self.state();
        state.resets.retain(|_, r| r.expires_at > now);
        state.resets.insert(reset.id, reset);
        Ok(())
    }

    async fn take_reset(&self, id: PasswordResetId) -> RepoResult<Option<PasswordReset>> {
        Ok(self.state().resets.remove(&id))
    }
}
