//! The password of local accounts, checked with OPAQUE.
//!
//! [`DefaultAccountPasswordService`] answers the password roots that stand on their own:
//! registration before a signup, the login start and finish, change, reset with the recovery
//! key, and setting a first password. A finished login issues a single-use ticket that the key
//! sign-in consumes. Storing the password a signup registered is a function of this module that
//! takes a repository, so `key_auth` runs it inside the signup's transaction.
//!
//! The server never sees the password. It stores the OPAQUE registration record and the
//! account's server share sealed under the password key, counts every login start as a wrong
//! try until a login finishes, and returns the share only to a login that proved the password.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use sea_orm::DatabaseConnection;
use tracing::{info, instrument, warn};
use uuid::Uuid;
use zeroize::Zeroizing;

use rim_auth::opaque::{
    LABEL_SERVER_SHARE, SESSION_KEY_LEN, server_login_finish, server_login_start,
    server_registration_finish, server_registration_start, transport_seal,
};
use rim_auth::session_token::{generate_challenge_nonce, generate_token, hash_token};
use rim_core::account_password::model::{
    AccountPassword, AttemptSchedule, AttemptStart, PasswordLogin, PasswordLoginId,
    PasswordLoginPurpose, PasswordReset, PasswordResetId,
};
use rim_core::account_password::repository::AccountPasswordRepository;
use rim_core::auth::repository::SessionRepository;
use rim_core::host::model::UserKind;
use rim_core::host::repository::HostRepository;
use rim_core::key_identity::model::KeyChallengePurpose;
use rim_core::key_identity::repository::{KeyChallengeRepository, KeyIdentityRepository};
use rim_core::repository::error::RepoErrorKind;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_identity::{KeyState, SignedStatement, UnverifiedPasswordReset, is_principal_id};
use rim_infra_db::{TransactionScope, begin_transaction};
use rim_services::account_password::dto::{
    KEPT_REWRAPS, MAX_PASSWORD_MESSAGE_LEN, PASSWORD_STEP_TTL_SECONDS, PasswordChangeFinishCmd,
    PasswordChangeStartCmd, PasswordChangeStartDto, PasswordChangedDto, PasswordLoginFinishCmd,
    PasswordLoginFinishDto, PasswordLoginStartCmd, PasswordLoginStartDto,
    PasswordRegistrationStartCmd, PasswordResetChallengeCmd, PasswordResetFinishCmd,
    PasswordResetStartCmd, PasswordResetStartDto, SetPasswordCmd, SetPasswordDto,
    SetPasswordStartCmd,
};
use rim_services::account_password::service::AccountPasswordService;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::key_auth::dto::KeyChallengeDto;
use rim_services::session_revocation::SessionRevocations;
use rim_services::{ServiceError, ServiceResult};

use crate::clock::Clock;
use crate::key_auth::{FAILURE_KEY, normalize_audience};
use crate::password_key::PasswordKey;
use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::repos::AppRepos;

/// Length of an account's server share.
pub(crate) const SERVER_SHARE_LEN: usize = 32;

/// Rate-limiter key of the anonymous password roots that issue something: registration starts
/// and reset challenges.
const ISSUE_KEY: &str = "password_issue";

/// The repositories the password service reads and writes.
#[derive(Clone)]
pub struct AccountPasswordRepos {
    pub password_repo: Arc<dyn AccountPasswordRepository>,
    pub user_repo: Arc<dyn UserRepository>,
    pub key_identity_repo: Arc<dyn KeyIdentityRepository>,
    pub key_challenge_repo: Arc<dyn KeyChallengeRepository>,
    pub session_repo: Arc<dyn SessionRepository>,
    /// Whether a principal's holder is a local account.
    pub host_repo: Arc<dyn HostRepository>,
}

/// A new id for a login state or a reset: random, so one id says nothing about another, and a
/// made-up id for an unknown principal looks like a real one.
fn random_id<T: From<Uuid>>() -> T {
    T::from(Uuid::new_v4())
}

/// `Validation` unless `bytes` is a non-empty message of at most
/// [`MAX_PASSWORD_MESSAGE_LEN`] bytes.
fn check_message(bytes: &[u8], field: &str) -> ServiceResult<()> {
    if bytes.is_empty() || bytes.len() > MAX_PASSWORD_MESSAGE_LEN {
        return Err(ServiceError::Validation(format!(
            "{field} must be 1 to {MAX_PASSWORD_MESSAGE_LEN} bytes"
        )));
    }
    Ok(())
}

/// `Validation` unless `principal_id` is a principal id.
fn check_principal(principal_id: &str) -> ServiceResult<()> {
    if is_principal_id(principal_id) {
        Ok(())
    } else {
        Err(ServiceError::Validation(
            "principalId is not a principal id".to_string(),
        ))
    }
}

fn malformed(field: &str) -> ServiceError {
    ServiceError::Validation(format!("{field} is not a valid OPAQUE message"))
}

/// The registration record a client's `RegistrationUpload` makes.
///
/// # Errors
/// `Validation` when the upload is empty, too long or not an OPAQUE registration upload.
pub(crate) fn registration_record(upload: &[u8]) -> ServiceResult<Vec<u8>> {
    check_message(upload, "passwordRegistration")?;
    server_registration_finish(upload).map_err(|_| malformed("passwordRegistration"))
}

/// Store a first password for `user_id` with a new server share, sealed under `key`. Returns
/// the share.
///
/// # Errors
/// `Conflict` when the account already has a password; storage and entropy failures.
pub(crate) async fn store_new_password(
    repo: &dyn AccountPasswordRepository,
    key: &PasswordKey,
    user_id: UserId,
    record: Vec<u8>,
    now: DateTime<Utc>,
) -> ServiceResult<Zeroizing<[u8; SERVER_SHARE_LEN]>> {
    let mut share = Zeroizing::new([0u8; SERVER_SHARE_LEN]);
    getrandom::fill(share.as_mut())
        .map_err(|e| ServiceError::internal(anyhow::anyhow!("entropy failure: {e}")))?;
    let sealed = key
        .seal_share(user_id, share.as_slice())
        .map_err(ServiceError::internal)?;
    repo.insert(user_id, record, sealed, now)
        .await
        .map_err(|e| match e.kind() {
            RepoErrorKind::Conflict => {
                ServiceError::Conflict("this account already has a password".to_string())
            }
            _ => e.into(),
        })?;
    Ok(share)
}

/// The server share of `password`, opened with `key`.
///
/// # Errors
/// `Internal` when it does not open: the password key was replaced.
pub(crate) fn open_share(
    key: &PasswordKey,
    password: &AccountPassword,
) -> ServiceResult<Zeroizing<Vec<u8>>> {
    key.open_share(password.user_id, &password.share_sealed)
        .ok_or_else(|| {
            ServiceError::internal(anyhow::anyhow!(
                "an account's server share does not open under the password key"
            ))
        })
}

/// Take the login state `login_id` for `purpose`, of `user_id` when one is given, and return
/// its account. The state is deleted whatever the outcome.
///
/// # Errors
/// `Unauthenticated` when the state is unknown, expired, of no account or another one, or for
/// another purpose.
async fn take_login(
    repo: &dyn AccountPasswordRepository,
    user_id: Option<UserId>,
    login_id: PasswordLoginId,
    purpose: PasswordLoginPurpose,
    now: DateTime<Utc>,
) -> ServiceResult<(UserId, PasswordLogin)> {
    let login = repo
        .take_login(login_id)
        .await?
        .filter(|l| l.purpose == purpose && l.expires_at > now)
        .ok_or(ServiceError::Unauthenticated)?;
    match login.user_id {
        Some(owner) if user_id.is_none_or(|u| u == owner) => Ok((owner, login)),
        _ => Err(ServiceError::Unauthenticated),
    }
}

/// The session key once the client's `finalization` verifies against `login`.
///
/// # Errors
/// `Unauthenticated` when it is malformed or does not verify: a wrong password.
fn verify_finalization(
    login: &PasswordLogin,
    finalization: &[u8],
) -> ServiceResult<Zeroizing<[u8; SESSION_KEY_LEN]>> {
    if finalization.is_empty() || finalization.len() > MAX_PASSWORD_MESSAGE_LEN {
        return Err(ServiceError::Unauthenticated);
    }
    server_login_finish(&login.state, finalization).map_err(|_| ServiceError::Unauthenticated)
}

/// Log a wrong password for `user_id`, with the tries counted so far.
pub(crate) fn audit_login_failure(user_id: UserId, failed_attempts: u32, purpose: &str) {
    info!(
        target: "audit",
        event = "password_login_failure",
        user_id = %user_id,
        failed_attempts,
        purpose,
        "a password did not verify"
    );
}

/// The verified key log head of `user_id`; `None` before enrollment or when the stored log no
/// longer verifies.
async fn key_state(
    repo: &dyn KeyIdentityRepository,
    user_id: UserId,
) -> ServiceResult<Option<KeyState>> {
    let entries = repo.list_key_log(user_id).await?;
    if entries.is_empty() {
        return Ok(None);
    }
    let statements: Vec<SignedStatement> = entries
        .into_iter()
        .map(|e| SignedStatement {
            payload: e.payload,
            signature: e.signature,
        })
        .collect();
    match KeyState::replay(&statements) {
        Ok(state) => Ok(Some(state)),
        Err(e) => {
            warn!(user_id = %user_id, error = %e, "stored key log does not verify");
            Ok(None)
        }
    }
}

pub struct DefaultAccountPasswordService {
    repos: AccountPasswordRepos,
    /// When set, every write that touches more than one row runs in one transaction.
    db: Option<DatabaseConnection>,
    /// `None` where the server loaded no password key: every root then answers `Internal`.
    key: Option<PasswordKey>,
    /// `host[:port]` values and the server id a reset's `audience` may name, normalized.
    accepted_audiences: Vec<String>,
    schedule: AttemptSchedule,
    /// Registration starts and reset challenges per IP.
    issue_rate_limiter: RegistrationRateLimiter,
    /// Login starts and failed resets per IP; the instance key sign-in failures count against.
    failure_rate_limiter: RegistrationRateLimiter,
    /// Told of the sessions a change or a reset ends, so their live connections close.
    session_revocations: Arc<dyn SessionRevocations>,
    clock: Clock,
}

impl DefaultAccountPasswordService {
    /// A service registering and checking passwords with `key`; a reset statement must name
    /// one of `accepted_audiences`.
    pub fn new(
        repos: AccountPasswordRepos,
        key: Option<PasswordKey>,
        accepted_audiences: Vec<String>,
    ) -> Self {
        Self {
            repos,
            db: None,
            key,
            accepted_audiences: accepted_audiences
                .into_iter()
                .map(|a| normalize_audience(a.trim()))
                .filter(|a| !a.is_empty())
                .collect(),
            schedule: AttemptSchedule::default(),
            issue_rate_limiter: RegistrationRateLimiter::new(300, 60),
            failure_rate_limiter: RegistrationRateLimiter::new(900, 20),
            session_revocations: crate::session_revocation::unshared(),
            clock: Clock::default(),
        }
    }

    /// Run each multi-row write in one transaction on `db`.
    pub fn with_database(mut self, db: DatabaseConnection) -> Self {
        self.db = Some(db);
        self
    }

    /// Count login starts and failed resets against `limiter`, shared with the key sign-in.
    pub fn with_failure_rate_limiter(mut self, limiter: RegistrationRateLimiter) -> Self {
        self.failure_rate_limiter = limiter;
        self
    }

    /// Report the sessions a change or a reset ends to `revocations`.
    pub fn with_session_revocations(mut self, revocations: Arc<dyn SessionRevocations>) -> Self {
        self.session_revocations = revocations;
        self
    }

    /// Read the time from `clock`.
    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// Replace the per-IP limiters (issued registrations and challenges, failures).
    #[cfg(test)]
    pub(crate) fn with_rate_limiters(
        mut self,
        issue: RegistrationRateLimiter,
        failure: RegistrationRateLimiter,
    ) -> Self {
        self.issue_rate_limiter = issue;
        self.failure_rate_limiter = failure;
        self
    }

    fn key(&self) -> ServiceResult<&PasswordKey> {
        self.key.as_ref().ok_or_else(|| {
            ServiceError::internal(anyhow::anyhow!("the password key is not loaded"))
        })
    }

    /// Open a transaction when a database is configured, and the repos that run inside it.
    async fn begin(&self) -> ServiceResult<(Option<TransactionScope>, AccountPasswordRepos)> {
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_account_password(),
            None => self.repos.clone(),
        };
        Ok((scope, repos))
    }

    async fn commit(scope: Option<TransactionScope>) -> ServiceResult<()> {
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        Ok(())
    }

    fn rate_limited(limiter_key: &'static str, retry_after_seconds: u64) -> ServiceError {
        crate::metrics::rate_limited(limiter_key);
        ServiceError::RateLimited {
            retry_after_seconds,
        }
    }

    /// Count a request against `limiter` under `key`; `RateLimited` once over the limit.
    /// Returns the instant recorded, which a refund must name.
    fn charge(
        limiter: &RegistrationRateLimiter,
        remote_ip: &str,
        key: &'static str,
        now: DateTime<Utc>,
    ) -> ServiceResult<DateTime<Utc>> {
        match limiter.check_and_record(remote_ip, key, now) {
            RateLimitDecision::Allowed => Ok(now),
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => Err(Self::rate_limited(key, retry_after_seconds)),
        }
    }

    /// The local account holding `principal_id`; `None` for nobody or an account of another
    /// server.
    async fn local_holder(&self, principal_id: &str) -> ServiceResult<Option<UserId>> {
        let Some(user_id) = self
            .repos
            .key_identity_repo
            .find_user_by_principal_id(principal_id)
            .await?
        else {
            return Ok(None);
        };
        Ok(
            (self.repos.host_repo.user_kind(user_id).await? == Some(UserKind::Local))
                .then_some(user_id),
        )
    }

    /// The signed-in local account's principal id.
    ///
    /// # Errors
    /// `Validation` for an account of another server or one without a key identity.
    async fn own_principal(&self, user_id: UserId) -> ServiceResult<String> {
        if self.repos.host_repo.user_kind(user_id).await? != Some(UserKind::Local) {
            return Err(ServiceError::Validation(
                "the password of an account lives at its own server".to_string(),
            ));
        }
        self.repos
            .user_repo
            .get_by_id(user_id)
            .await?
            .and_then(|u| u.principal_id)
            .ok_or_else(|| ServiceError::Validation("this account has no key identity".to_string()))
    }

    /// Count a try against `user_id`'s password; the record to answer it with.
    ///
    /// # Errors
    /// `PasswordLocked`, `RateLimited` while a wait runs, `NotFound` without a password.
    async fn count_try(&self, user_id: UserId, now: DateTime<Utc>) -> ServiceResult<Vec<u8>> {
        match self
            .repos
            .password_repo
            .start_attempt(user_id, now, &self.schedule)
            .await?
        {
            AttemptStart::Counted {
                registration,
                failed_attempts,
            } => {
                if self.schedule.locks_at(failed_attempts) {
                    info!(
                        target: "audit",
                        event = "password_locked",
                        user_id = %user_id,
                        failed_attempts,
                        "a password locked after too many tries"
                    );
                }
                Ok(registration)
            }
            AttemptStart::Locked => Err(ServiceError::PasswordLocked),
            AttemptStart::Wait { until } => Err(Self::rate_limited(
                "password_try",
                u64::try_from((until - now).num_seconds().max(1)).unwrap_or(1),
            )),
            AttemptStart::NoPassword => Err(ServiceError::NotFound),
        }
    }

    /// Store an OPAQUE login state for `user_id` and return its id.
    async fn store_login(
        &self,
        user_id: Option<UserId>,
        purpose: PasswordLoginPurpose,
        state: Vec<u8>,
        now: DateTime<Utc>,
    ) -> ServiceResult<PasswordLoginId> {
        let id = random_id();
        self.repos
            .password_repo
            .insert_login(
                PasswordLogin {
                    id,
                    user_id,
                    purpose,
                    state,
                    expires_at: now + Duration::seconds(PASSWORD_STEP_TTL_SECONDS),
                },
                now,
            )
            .await?;
        Ok(id)
    }

    /// Log a wrong password for `user_id` with the tries counted so far.
    async fn audit_failure(&self, user_id: UserId, purpose: &str) -> ServiceResult<()> {
        let tries = self
            .repos
            .password_repo
            .get(user_id)
            .await?
            .map_or(0, |p| p.failed_attempts);
        audit_login_failure(user_id, tries, purpose);
        Ok(())
    }

    /// Whether a reset statement's `audience` names this server.
    fn audience_accepted(&self, audience: &str) -> bool {
        self.accepted_audiences
            .contains(&normalize_audience(audience))
    }

    /// The account and registration response of a verified reset; see
    /// [`AccountPasswordService::reset_start`].
    async fn verify_reset(
        &self,
        cmd: &PasswordResetStartCmd,
        now: DateTime<Utc>,
    ) -> ServiceResult<PasswordResetStartDto> {
        let fail = |reason: &str| {
            info!(
                target: "audit",
                event = "password_reset_failure",
                reason,
                "password reset refused"
            );
            ServiceError::Unauthenticated
        };
        let statement = SignedStatement {
            payload: cmd.statement.payload.clone(),
            signature: cmd.statement.signature.clone(),
        };
        let unverified =
            UnverifiedPasswordReset::read(&statement).map_err(crate::util::statement_refused)?;
        check_message(&cmd.registration_request, "registrationRequest")?;
        let Some(user_id) = self.local_holder(&unverified.principal_id).await? else {
            return Err(fail("unknown_principal"));
        };
        let Some(state) = key_state(self.repos.key_identity_repo.as_ref(), user_id).await? else {
            return Err(fail("no_key_log"));
        };
        let proof = unverified
            .verify(&state)
            .map_err(|_| fail("bad_signature"))?;
        if !self.audience_accepted(&proof.audience) {
            return Err(fail("audience_mismatch"));
        }
        let registration_response = server_registration_start(
            self.key()?.setup(),
            &cmd.registration_request,
            proof.principal_id.as_bytes(),
        )
        .map_err(|_| malformed("registrationRequest"))?;
        if !self
            .repos
            .key_challenge_repo
            .consume(
                proof.challenge_id.into(),
                &proof.nonce,
                &KeyChallengePurpose::PasswordReset,
            )
            .await?
        {
            return Err(fail("challenge_invalid"));
        }
        let reset_id: PasswordResetId = random_id();
        self.repos
            .password_repo
            .insert_reset(
                PasswordReset {
                    id: reset_id,
                    user_id,
                    expires_at: now + Duration::seconds(PASSWORD_STEP_TTL_SECONDS),
                },
                now,
            )
            .await?;
        Ok(PasswordResetStartDto {
            reset_id,
            registration_response,
        })
    }
}

#[async_trait]
impl AccountPasswordService for DefaultAccountPasswordService {
    #[instrument(name = "account_password.registration_start", skip_all)]
    async fn registration_start(
        &self,
        cmd: PasswordRegistrationStartCmd,
    ) -> ServiceResult<Vec<u8>> {
        let now = self.clock.now();
        Self::charge(&self.issue_rate_limiter, &cmd.remote_ip, ISSUE_KEY, now)?;
        check_principal(&cmd.principal_id)?;
        check_message(&cmd.request, "request")?;
        if self.local_holder(&cmd.principal_id).await?.is_some() {
            return Err(ServiceError::Conflict(
                "another account on this server already uses this key identity".to_string(),
            ));
        }
        server_registration_start(
            self.key()?.setup(),
            &cmd.request,
            cmd.principal_id.as_bytes(),
        )
        .map_err(|_| malformed("request"))
    }

    #[instrument(name = "account_password.login_start", skip_all)]
    async fn login_start(
        &self,
        cmd: PasswordLoginStartCmd,
    ) -> ServiceResult<PasswordLoginStartDto> {
        let now = self.clock.now();
        // Every start may test one guess, so every start is charged and none is refunded.
        Self::charge(&self.failure_rate_limiter, &cmd.remote_ip, FAILURE_KEY, now)?;
        check_principal(&cmd.principal_id)?;
        check_message(&cmd.request, "request")?;
        let setup = self.key()?.setup();
        let credential_id = cmd.principal_id.as_bytes();
        let user_id = self.local_holder(&cmd.principal_id).await?;
        let record = match user_id {
            Some(user_id) => match self.count_try(user_id, now).await {
                Ok(record) => Some((user_id, record)),
                Err(ServiceError::NotFound) => None,
                Err(e) => return Err(e),
            },
            None => None,
        };
        match record {
            Some((user_id, record)) => {
                let started = server_login_start(setup, Some(&record), &cmd.request, credential_id)
                    .map_err(|_| malformed("request"))?;
                let login_id = self
                    .store_login(
                        Some(user_id),
                        PasswordLoginPurpose::Login,
                        started.state.to_vec(),
                        now,
                    )
                    .await?;
                Ok(PasswordLoginStartDto {
                    login_id,
                    response: started.response,
                })
            }
            // An answer of the same shape that no password finishes, and an id that names no
            // state, so the answer does not tell whether the principal has a password here.
            None => {
                let started = server_login_start(setup, None, &cmd.request, credential_id)
                    .map_err(|_| malformed("request"))?;
                Ok(PasswordLoginStartDto {
                    login_id: random_id(),
                    response: started.response,
                })
            }
        }
    }

    #[instrument(name = "account_password.login_finish", skip_all)]
    async fn login_finish(
        &self,
        cmd: PasswordLoginFinishCmd,
    ) -> ServiceResult<PasswordLoginFinishDto> {
        let now = self.clock.now();
        let (user_id, login) = take_login(
            self.repos.password_repo.as_ref(),
            None,
            cmd.login_id,
            PasswordLoginPurpose::Login,
            now,
        )
        .await?;
        let session_key = match verify_finalization(&login, &cmd.credential_finalization) {
            Ok(key) => key,
            Err(e) => {
                self.audit_failure(user_id, "login").await?;
                return Err(e);
            }
        };
        let key = self.key()?;
        let password = self
            .repos
            .password_repo
            .get(user_id)
            .await?
            .ok_or(ServiceError::Unauthenticated)?;
        let share = open_share(key, &password)?;
        let server_share = transport_seal(session_key.as_slice(), LABEL_SERVER_SHARE, &share)
            .map_err(ServiceError::internal)?;
        let ticket = generate_token("");
        self.repos
            .password_repo
            .clear_attempts(user_id, now)
            .await?;
        self.repos
            .password_repo
            .insert_ticket(
                &hash_token(&ticket),
                user_id,
                now + Duration::seconds(PASSWORD_STEP_TTL_SECONDS),
                now,
            )
            .await?;
        let rewraps = self.repos.password_repo.list_rewraps(user_id).await?;
        info!(
            target: "audit",
            event = "password_login_success",
            user_id = %user_id,
            "a password verified; a sign-in ticket was issued"
        );
        Ok(PasswordLoginFinishDto {
            ticket,
            server_share,
            rewraps,
        })
    }

    #[instrument(name = "account_password.change_start", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn change_start(
        &self,
        cmd: PasswordChangeStartCmd,
    ) -> ServiceResult<PasswordChangeStartDto> {
        let user_id = cmd.actor.user_id;
        let now = self.clock.now();
        check_message(&cmd.credential_request, "credentialRequest")?;
        check_message(&cmd.registration_request, "registrationRequest")?;
        let principal_id = self.own_principal(user_id).await?;
        let setup = self.key()?.setup();
        let registration_response =
            server_registration_start(setup, &cmd.registration_request, principal_id.as_bytes())
                .map_err(|_| malformed("registrationRequest"))?;
        let record = self.count_try(user_id, now).await?;
        let started = server_login_start(
            setup,
            Some(&record),
            &cmd.credential_request,
            principal_id.as_bytes(),
        )
        .map_err(|_| malformed("credentialRequest"))?;
        let login_id = self
            .store_login(
                Some(user_id),
                PasswordLoginPurpose::Change,
                started.state.to_vec(),
                now,
            )
            .await?;
        Ok(PasswordChangeStartDto {
            login_id,
            credential_response: started.response,
            registration_response,
        })
    }

    #[instrument(name = "account_password.change_finish", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn change_finish(
        &self,
        cmd: PasswordChangeFinishCmd,
    ) -> ServiceResult<PasswordChangedDto> {
        let user_id = cmd.actor.user_id;
        let now = self.clock.now();
        let record = registration_record(&cmd.registration_upload)?;
        check_message(&cmd.rewrap, "rewrap")?;
        let (_, login) = take_login(
            self.repos.password_repo.as_ref(),
            Some(user_id),
            cmd.login_id,
            PasswordLoginPurpose::Change,
            now,
        )
        .await?;
        if let Err(e) = verify_finalization(&login, &cmd.credential_finalization) {
            self.audit_failure(user_id, "change").await?;
            return Err(e);
        }

        let (scope, repos) = self.begin().await?;
        if !repos.key_identity_repo.lock_user(user_id).await? {
            return Err(ServiceError::NotFound);
        }
        if !repos
            .password_repo
            .replace_registration(user_id, record, now)
            .await?
        {
            return Err(ServiceError::NotFound);
        }
        repos
            .password_repo
            .insert_rewrap(user_id, cmd.rewrap, now, KEPT_REWRAPS)
            .await?;
        let ended_sessions = repos
            .session_repo
            .revoke_all_for_user_except(user_id, cmd.session_id)
            .await?;
        Self::commit(scope).await?;
        self.session_revocations
            .revoke_user_except(user_id, cmd.session_id);
        info!(
            target: "audit",
            event = "password_changed",
            user_id = %user_id,
            session_id = %cmd.session_id,
            ended_sessions,
            "password changed; other sessions ended"
        );
        Ok(PasswordChangedDto { changed_at: now })
    }

    #[instrument(name = "account_password.reset_challenge", skip_all)]
    async fn reset_challenge(
        &self,
        cmd: PasswordResetChallengeCmd,
    ) -> ServiceResult<KeyChallengeDto> {
        let now = self.clock.now();
        Self::charge(&self.issue_rate_limiter, &cmd.remote_ip, ISSUE_KEY, now)?;
        check_principal(&cmd.principal_id)?;
        // Best effort: an old row left behind costs nothing but space.
        if let Err(e) = self.repos.key_challenge_repo.delete_expired(now).await {
            warn!(error = %e, "could not delete expired key challenges");
        }
        let challenge = self
            .repos
            .key_challenge_repo
            .create(
                &generate_challenge_nonce(),
                now + Duration::seconds(PASSWORD_STEP_TTL_SECONDS),
                &KeyChallengePurpose::PasswordReset,
            )
            .await?;
        Ok(KeyChallengeDto {
            challenge_id: challenge.id,
            nonce: challenge.nonce,
            expires_at: challenge.expires_at,
        })
    }

    #[instrument(name = "account_password.reset_start", skip_all)]
    async fn reset_start(
        &self,
        cmd: PasswordResetStartCmd,
    ) -> ServiceResult<PasswordResetStartDto> {
        let now = self.clock.now();
        let reserved_at =
            Self::charge(&self.failure_rate_limiter, &cmd.remote_ip, FAILURE_KEY, now)?;
        let result = self.verify_reset(&cmd, now).await;
        if !matches!(result, Err(ServiceError::Unauthenticated)) {
            self.failure_rate_limiter
                .refund(&cmd.remote_ip, FAILURE_KEY, reserved_at);
        }
        result
    }

    #[instrument(name = "account_password.reset_finish", skip_all)]
    async fn reset_finish(&self, cmd: PasswordResetFinishCmd) -> ServiceResult<()> {
        let now = self.clock.now();
        let record = registration_record(&cmd.registration_upload)?;
        let key = self.key()?;
        let reset = self
            .repos
            .password_repo
            .take_reset(cmd.reset_id)
            .await?
            .filter(|r| r.expires_at > now)
            .ok_or(ServiceError::Unauthenticated)?;
        let user_id = reset.user_id;

        let (scope, repos) = self.begin().await?;
        if !repos.key_identity_repo.lock_user(user_id).await? {
            return Err(ServiceError::Unauthenticated);
        }
        let replaced = repos
            .password_repo
            .replace_registration(user_id, record.clone(), now)
            .await?;
        if !replaced {
            store_new_password(repos.password_repo.as_ref(), key, user_id, record, now).await?;
        }
        let rewraps = repos.password_repo.delete_rewraps(user_id).await?;
        repos
            .session_repo
            .clear_password_requirement(user_id)
            .await?;
        let ended_sessions = repos.session_repo.revoke_all_for_user(user_id).await?;
        Self::commit(scope).await?;
        self.session_revocations.revoke_user(user_id);
        info!(
            target: "audit",
            event = "password_reset",
            user_id = %user_id,
            had_password = replaced,
            rewraps,
            ended_sessions,
            "password reset with the recovery key; every session ended"
        );
        Ok(())
    }

    #[instrument(name = "account_password.set_password_start", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn set_password_start(&self, cmd: SetPasswordStartCmd) -> ServiceResult<Vec<u8>> {
        check_message(&cmd.request, "request")?;
        let principal_id = self.own_principal(cmd.actor.user_id).await?;
        if self
            .repos
            .password_repo
            .get(cmd.actor.user_id)
            .await?
            .is_some()
        {
            return Err(ServiceError::Conflict(
                "this account already has a password".to_string(),
            ));
        }
        server_registration_start(self.key()?.setup(), &cmd.request, principal_id.as_bytes())
            .map_err(|_| malformed("request"))
    }

    #[instrument(name = "account_password.set_password", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn set_password(&self, cmd: SetPasswordCmd) -> ServiceResult<SetPasswordDto> {
        let user_id = cmd.actor.user_id;
        let now = self.clock.now();
        let record = registration_record(&cmd.upload)?;
        self.own_principal(user_id).await?;
        let key = self.key()?;

        let (scope, repos) = self.begin().await?;
        let share =
            store_new_password(repos.password_repo.as_ref(), key, user_id, record, now).await?;
        let lifted = repos
            .session_repo
            .clear_password_requirement(user_id)
            .await?;
        Self::commit(scope).await?;
        info!(
            target: "audit",
            event = "password_set",
            user_id = %user_id,
            lifted_sessions = lifted,
            "password set for an account that had none"
        );
        Ok(SetPasswordDto {
            server_share: share.to_vec(),
        })
    }
}
