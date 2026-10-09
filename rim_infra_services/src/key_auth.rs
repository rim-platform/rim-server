//! Key identity and challenge login (Stage 3, Track 7.2). Contract:
//! `docs/specs/key-identity-v1.md`. Statement verification lives in `rim_identity`; this module
//! decides who may submit what, stores what verified, and issues sessions.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use sea_orm::DatabaseConnection;
use tracing::{info, instrument, warn};

use rim_auth::csrf::generate_csrf_token;
use rim_auth::session_token::{generate_challenge_nonce, generate_session_token, hash_token};
use rim_core::auth::model::NewSession;
use rim_core::auth::repository::SessionRepository;
use rim_core::key_identity::model::{
    DeviceKey, KeyLogEntry, KeyLogEntryType, NewDeviceKey, NewKeyLogEntry, StoredProfileCard,
};
use rim_core::key_identity::repository::{KeyChallengeRepository, KeyIdentityRepository};
use rim_core::repository::error::RepoErrorKind;
use rim_core::user::model::{NewUser, NewUserKeyIdentity, UserId};
use rim_core::user::repository::UserRepository;
use rim_identity::{
    DeviceCert, IdentityError, KeyState, LoginProof, PublicKey, RotationSigner, SignedStatement,
    UnverifiedLogin, UnverifiedRevocation, rotation_principal_id, verify_device_cert,
    verify_profile_card,
};
use rim_infra_db::{TransactionScope, begin_transaction};
use rim_services::auth::dto::{AuthSessionDto, LoginResultDto, RegistrationMode, SignupResultDto};
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::events::Observers;
use rim_services::key_auth::command::{
    AddDeviceKeyCmd, EnrollIdentityKeyCmd, GetKeyLogQuery, GetMyKeyIdentityQuery,
    IssueKeyChallengeCmd, LoginWithKeyCmd, PublishProfileCardCmd, RevokeDeviceKeyCmd,
    RotateIdentityKeyCmd, SignedPayload, SignupWithKeyCmd,
};
use rim_services::key_auth::dto::{
    DeviceKeyDto, KeyChallengeDto, KeyIdentityDto, KeyLogEntryDto, ProfileCardDto,
};
use rim_services::key_auth::service::KeyAuthService;
use rim_services::{ServiceError, ServiceResult};

use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::repos::AppRepos;
use crate::signup::{self, SignupRepos};

/// How long a login challenge stays valid.
const CHALLENGE_TTL_SECONDS: i64 = 120;

/// Failure-limiter key shared by key logins and unauthenticated recoveries.
const FAILURE_KEY: &str = "key_auth_failure";

/// Most entries a key log may have: at enrollment and key-only signup (every entry is verified
/// before the transaction starts, so the bound keeps one request's work small), and grown by
/// identity rotations, so that a log they grew can be enrolled on another server. Recoveries
/// may pass it.
const MAX_KEY_LOG_LEN: usize = 256;

/// Longest `principalId` the public key log query looks up (a real one is 49 characters).
const MAX_PRINCIPAL_ID_LEN: usize = 64;

#[derive(Clone)]
pub struct KeyAuthRepos {
    pub user_repo: Arc<dyn UserRepository>,
    pub session_repo: Arc<dyn SessionRepository>,
    pub key_identity_repo: Arc<dyn KeyIdentityRepository>,
    pub key_challenge_repo: Arc<dyn KeyChallengeRepository>,
    /// Registration tokens and workspace joins, for key-only signup.
    pub signup: SignupRepos,
}

/// An audience as compared: a server id (`rims1_…`) as it is, since base64url is
/// case-sensitive; a `host[:port]` lowercased. Host names cannot contain `_`, so the prefix is
/// unambiguous.
fn normalize_audience(audience: &str) -> String {
    if rim_s2s::is_server_id(audience) {
        audience.to_string()
    } else {
        audience.to_ascii_lowercase()
    }
}

pub struct DefaultKeyAuthService {
    repos: KeyAuthRepos,
    /// When set, every write runs in one transaction with the user row locked.
    db: Option<DatabaseConnection>,
    session_ttl: Duration,
    /// `host[:port]` values a login proof's `audience` may name, lowercased.
    accepted_audiences: Vec<String>,
    /// Challenges issued per IP.
    challenge_rate_limiter: RegistrationRateLimiter,
    /// Failed key logins and recovery attempts per IP.
    failure_rate_limiter: RegistrationRateLimiter,
    /// Open-mode signups per IP; the same instance the password signup counts against.
    signup_rate_limiter: RegistrationRateLimiter,
    /// Announces a key-only signup through a workspace invite (`MemberJoined`).
    observers: Observers,
    /// Told when a user's devices change, so their encrypted groups are updated. `None` in
    /// unit tests; the composition root wires it.
    e2ee_service: Option<Arc<dyn rim_services::e2ee::service::E2eeService>>,
}

/// A key identity a device presented to enroll or sign up with: verified, not yet stored.
struct PresentedIdentity {
    state: KeyState,
    cert: DeviceCert,
    /// Its challenge is consumed inside the caller's transaction.
    proof: LoginProof,
    /// The log as it is stored; entry 0 is the inception.
    entries: Vec<NewKeyLogEntry>,
}

fn to_statement(p: &SignedPayload) -> SignedStatement {
    SignedStatement {
        payload: p.payload.clone(),
        signature: p.signature.clone(),
    }
}

fn invalid(err: IdentityError) -> ServiceError {
    ServiceError::Validation(err.to_string())
}

fn log_entry_dto(e: KeyLogEntry) -> KeyLogEntryDto {
    KeyLogEntryDto {
        seq: e.seq,
        entry_type: e.entry_type,
        payload: e.payload,
        signature: e.signature,
        created_at: e.created_at,
    }
}

fn device_dto(d: DeviceKey) -> DeviceKeyDto {
    DeviceKeyDto {
        device_id: d.device_id,
        public_key: d.public_key,
        name: d.name,
        key_seq: d.key_seq,
        cert_payload: d.cert_payload,
        cert_signature: d.cert_signature,
        added_at: d.added_at,
        last_used_at: d.last_used_at,
        revoked_at: d.revoked_at,
    }
}

fn card_dto(c: StoredProfileCard) -> ProfileCardDto {
    ProfileCardDto {
        version: c.version,
        payload: c.payload,
        signature: c.signature,
        updated_at: c.updated_at,
    }
}

fn new_device(user_id: UserId, cert: &DeviceCert, statement: &SignedPayload) -> NewDeviceKey {
    NewDeviceKey {
        user_id,
        device_id: cert.device_id,
        public_key: cert.device_key.to_string(),
        key_hash: cert.device_key.key_hash().to_vec(),
        name: cert.name.clone(),
        cert_payload: statement.payload.clone(),
        cert_signature: statement.signature.clone(),
        key_seq: cert.key_seq,
    }
}

/// The user's verified key log head, or `None` before enrollment.
///
/// The server verified every entry before storing it, so a log that no longer verifies means the
/// stored rows were changed by hand: an internal error, never the caller's fault.
async fn load_state(repos: &KeyAuthRepos, user_id: UserId) -> ServiceResult<Option<KeyState>> {
    let entries = repos.key_identity_repo.list_key_log(user_id).await?;
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
    KeyState::replay(&statements).map(Some).map_err(|e| {
        warn!(user_id = %user_id, error = %e, "stored key log does not verify");
        ServiceError::internal(anyhow::anyhow!("stored key log does not verify: {e}"))
    })
}

/// Insert a device, turning a duplicate into a message the client can act on.
async fn insert_device(repos: &KeyAuthRepos, device: NewDeviceKey) -> ServiceResult<DeviceKey> {
    repos
        .key_identity_repo
        .add_device(device)
        .await
        .map_err(|e| match e.kind() {
            RepoErrorKind::Conflict => ServiceError::Conflict(
                "this device id or device key is already enrolled or was revoked".to_string(),
            ),
            _ => e.into(),
        })
}

impl DefaultKeyAuthService {
    /// Build the service. `signup_rate_limiter` is the instance the password signup uses, so
    /// both signups share one per-IP budget.
    pub fn new(
        repos: KeyAuthRepos,
        db: Option<DatabaseConnection>,
        session_ttl: Duration,
        accepted_audiences: Vec<String>,
        observers: Observers,
        signup_rate_limiter: RegistrationRateLimiter,
    ) -> Self {
        Self {
            repos,
            db,
            session_ttl,
            accepted_audiences: accepted_audiences
                .into_iter()
                .map(|a| normalize_audience(a.trim()))
                .filter(|a| !a.is_empty())
                .collect(),
            challenge_rate_limiter: RegistrationRateLimiter::new(300, 60),
            failure_rate_limiter: RegistrationRateLimiter::new(900, 20),
            signup_rate_limiter,
            observers,
            e2ee_service: None,
        }
    }

    pub fn with_e2ee_service(
        mut self,
        e2ee_service: Arc<dyn rim_services::e2ee::service::E2eeService>,
    ) -> Self {
        self.e2ee_service = Some(e2ee_service);
        self
    }

    /// Best-effort: the user's encrypted groups may now have devices to remove.
    async fn devices_changed(&self, user_id: UserId) {
        if let Some(e2ee) = &self.e2ee_service
            && let Err(e) = e2ee.devices_changed(user_id).await
        {
            warn!(error = %e, user_id = %user_id, "could not announce a device change to encrypted conversations");
        }
    }

    /// Replace the per-IP limiters (challenges issued, failed logins and recoveries).
    #[cfg(test)]
    pub(crate) fn with_rate_limiters(
        mut self,
        challenge: RegistrationRateLimiter,
        failure: RegistrationRateLimiter,
    ) -> Self {
        self.challenge_rate_limiter = challenge;
        self.failure_rate_limiter = failure;
        self
    }

    /// Open a transaction when a database is configured, and the repos that run inside it.
    async fn begin(&self) -> ServiceResult<(Option<TransactionScope>, KeyAuthRepos)> {
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_key_auth(),
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

    /// Lock the user's row and load their key log head; `Validation` when they have no key.
    async fn lock_and_load(repos: &KeyAuthRepos, user_id: UserId) -> ServiceResult<KeyState> {
        if !repos.key_identity_repo.lock_user(user_id).await? {
            return Err(ServiceError::NotFound);
        }
        load_state(repos, user_id).await?.ok_or_else(|| {
            ServiceError::Validation("this account has no key identity yet".to_string())
        })
    }

    /// Whether a proof's `audience` names this server: one of its `host[:port]` values,
    /// compared case-insensitively, or its server id, compared exactly.
    fn audience_accepted(&self, audience: &str) -> bool {
        self.accepted_audiences
            .contains(&normalize_audience(audience))
    }

    /// Count an attempt against the caller's failure budget; `RateLimited` once over the limit.
    /// Returns the instant recorded, which a refund must name.
    fn reserve_failure(&self, remote_ip: &str) -> ServiceResult<DateTime<Utc>> {
        let now = Utc::now();
        match self
            .failure_rate_limiter
            .check_and_record(remote_ip, FAILURE_KEY, now)
        {
            RateLimitDecision::Allowed => Ok(now),
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => Err(ServiceError::RateLimited {
                retry_after_seconds,
            }),
        }
    }

    /// Verify an identity presented for enrollment or key-only signup, without touching
    /// storage: the whole log (at most 256 entries) replays from its inception, the certificate
    /// is signed by the head identity key, and the proof is a `login` statement by that device
    /// naming this log's principal, the certificate's device id and this server's audience. The
    /// proof's challenge is not consumed here; see [`Self::consume_proof_challenge`].
    ///
    /// # Errors
    /// `Validation` naming what does not verify.
    fn verify_presented(
        &self,
        key_log: &[SignedPayload],
        device: &SignedPayload,
        proof: &SignedPayload,
    ) -> ServiceResult<PresentedIdentity> {
        if key_log.len() > MAX_KEY_LOG_LEN {
            return Err(ServiceError::Validation(format!(
                "a key log may have at most {MAX_KEY_LOG_LEN} entries"
            )));
        }
        let statements: Vec<SignedStatement> = key_log.iter().map(to_statement).collect();
        let state = KeyState::replay(&statements).map_err(invalid)?;
        let cert = verify_device_cert(&state, &to_statement(device)).map_err(invalid)?;
        // Proof that the caller holds the device key, over a challenge from this server. The log
        // and certificate are public and portable, so without it anyone who has seen them could
        // claim someone else's principal on another server.
        let unverified = UnverifiedLogin::read(&to_statement(proof)).map_err(invalid)?;
        if unverified.principal_id != state.principal_id || unverified.device_id != cert.device_id {
            return Err(ServiceError::Validation(
                "the proof must be signed by the device being enrolled".to_string(),
            ));
        }
        let proof = unverified.verify(&cert.device_key).map_err(invalid)?;
        if !self.audience_accepted(&proof.audience) {
            return Err(ServiceError::Validation(
                "the proof names another server as its audience".to_string(),
            ));
        }
        // `replay` checked that entry 0 is the inception and every later entry a rotation.
        let entries = statements
            .into_iter()
            .zip(0u32..)
            .map(|(s, seq)| NewKeyLogEntry {
                seq,
                entry_type: if seq == 0 {
                    KeyLogEntryType::Inception
                } else {
                    KeyLogEntryType::Rotate
                },
                payload: s.payload,
                signature: s.signature,
            })
            .collect();
        Ok(PresentedIdentity {
            state,
            cert,
            proof,
            entries,
        })
    }

    /// Claim the challenge a presented identity's proof signed, inside the caller's
    /// transaction (a rollback gives it back).
    ///
    /// # Errors
    /// `Validation` when the challenge is unknown, already used or expired.
    async fn consume_proof_challenge(
        repos: &KeyAuthRepos,
        proof: &LoginProof,
    ) -> ServiceResult<()> {
        if !repos
            .key_challenge_repo
            .consume(proof.challenge_id.into(), &proof.nonce)
            .await?
        {
            return Err(ServiceError::Validation(
                "the challenge is unknown, already used or expired".to_string(),
            ));
        }
        Ok(())
    }

    /// `Conflict` when an account on this server already has `principal_id`.
    async fn ensure_principal_unused(
        repos: &KeyAuthRepos,
        principal_id: &str,
    ) -> ServiceResult<()> {
        if repos
            .key_identity_repo
            .find_user_by_principal_id(principal_id)
            .await?
            .is_some()
        {
            return Err(ServiceError::Conflict(
                "another account on this server already uses this key identity".to_string(),
            ));
        }
        Ok(())
    }

    /// Verify a login proof and claim its challenge. Every refusal is `Unauthenticated` except
    /// a valid signature from a revoked device.
    async fn verify_login(&self, proof: &SignedPayload) -> ServiceResult<(UserId, DeviceKey)> {
        let fail = |reason: &str| {
            info!(
                target = "audit",
                event = "key_login_failure",
                reason,
                "key login failed"
            );
            ServiceError::Unauthenticated
        };
        let unverified =
            UnverifiedLogin::read(&to_statement(proof)).map_err(|_| fail("malformed_proof"))?;
        let Some(user_id) = self
            .repos
            .key_identity_repo
            .find_user_by_principal_id(&unverified.principal_id)
            .await?
        else {
            return Err(fail("unknown_principal"));
        };
        let Some(device) = self
            .repos
            .key_identity_repo
            .get_device(user_id, unverified.device_id)
            .await?
        else {
            return Err(fail("unknown_device"));
        };
        let device_key = PublicKey::parse(&device.public_key, "deviceKey")
            .map_err(|e| ServiceError::internal(anyhow::anyhow!("stored device key: {e}")))?;
        let login = unverified
            .verify(&device_key)
            .map_err(|_| fail("bad_signature"))?;
        // Revocation is answered only for a proof that would otherwise pass, so an old proof,
        // or one made for another server, never tells whether the device was revoked.
        if !self.audience_accepted(&login.audience) {
            return Err(fail("audience_mismatch"));
        }
        if !self
            .repos
            .key_challenge_repo
            .consume(login.challenge_id.into(), &login.nonce)
            .await?
        {
            return Err(fail("challenge_invalid"));
        }
        if device.revoked_at.is_some() {
            info!(
                target = "audit",
                event = "key_login_failure",
                reason = "device_revoked",
                user_id = %user_id,
                device_id = %device.device_id,
                "key login refused: device revoked"
            );
            return Err(ServiceError::PermissionDenied);
        }
        Ok((user_id, device))
    }
}

#[async_trait]
impl KeyAuthService for DefaultKeyAuthService {
    #[instrument(name = "key_auth.issue_challenge", skip_all)]
    async fn issue_challenge(&self, cmd: IssueKeyChallengeCmd) -> ServiceResult<KeyChallengeDto> {
        let now = Utc::now();
        if let RateLimitDecision::Limited {
            retry_after_seconds,
        } = self
            .challenge_rate_limiter
            .check_and_record(&cmd.remote_ip, "key_challenge", now)
        {
            return Err(ServiceError::RateLimited {
                retry_after_seconds,
            });
        }
        // Best effort: an old row left behind costs nothing but space.
        if let Err(e) = self.repos.key_challenge_repo.delete_expired(now).await {
            warn!(error = %e, "could not delete expired key challenges");
        }
        let nonce = generate_challenge_nonce();
        let challenge = self
            .repos
            .key_challenge_repo
            .create(&nonce, now + Duration::seconds(CHALLENGE_TTL_SECONDS))
            .await?;
        Ok(KeyChallengeDto {
            challenge_id: challenge.id,
            nonce: challenge.nonce,
            expires_at: challenge.expires_at,
        })
    }

    #[instrument(name = "key_auth.login", skip_all)]
    async fn login_with_key(&self, cmd: LoginWithKeyCmd) -> ServiceResult<LoginResultDto> {
        // A slot is reserved up front, so concurrent attempts can't all pass the check before
        // any of them fails, and refunded unless the attempt turns out to be a failed login.
        let reserved_at = self.reserve_failure(&cmd.remote_ip)?;
        let verified = self.verify_login(&cmd.proof).await;
        if !matches!(
            verified,
            Err(ServiceError::Unauthenticated | ServiceError::PermissionDenied)
        ) {
            self.failure_rate_limiter
                .refund(&cmd.remote_ip, FAILURE_KEY, reserved_at);
        }
        let (user_id, device) = verified?;

        // A revocation (or recovery) commits under the user row lock. Taking the same lock and
        // reading the device again means a revocation that raced the checks above either ends
        // this session too or is seen here; it never leaves a live session on a revoked device.
        let (scope, repos) = self.begin().await?;
        if !repos.key_identity_repo.lock_user(user_id).await? {
            return Err(ServiceError::Unauthenticated);
        }
        let current = repos
            .key_identity_repo
            .get_device(user_id, device.device_id)
            .await?;
        if current.is_none_or(|d| d.revoked_at.is_some()) {
            info!(
                target = "audit",
                event = "key_login_failure",
                reason = "device_revoked",
                user_id = %user_id,
                device_id = %device.device_id,
                "key login refused: device revoked during login"
            );
            return Err(ServiceError::PermissionDenied);
        }
        let raw_token = generate_session_token();
        let session = repos
            .session_repo
            .create_session(NewSession {
                user_id,
                token_hash: hash_token(&raw_token),
                expires_at: Utc::now() + self.session_ttl,
                user_agent: None,
                ip: None,
            })
            .await?;
        repos
            .session_repo
            .bind_device(session.id, device.id)
            .await?;
        repos.key_identity_repo.touch_device(device.id).await?;
        Self::commit(scope).await?;

        info!(
            target = "audit",
            event = "key_login_success",
            user_id = %user_id,
            device_id = %device.device_id,
            session_id = %session.id,
            "key login succeeded"
        );
        let csrf_token = generate_csrf_token(&raw_token);
        Ok(LoginResultDto {
            user_id,
            session: AuthSessionDto {
                user_id,
                session_id: session.id,
                expires_at: session.expires_at,
            },
            session_token: raw_token,
            csrf_token,
        })
    }

    #[instrument(name = "key_auth.enroll", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn enroll(&self, cmd: EnrollIdentityKeyCmd) -> ServiceResult<KeyIdentityDto> {
        let user_id = cmd.actor.user_id;
        let PresentedIdentity {
            state,
            cert,
            proof,
            entries,
        } = self.verify_presented(&cmd.key_log, &cmd.device, &cmd.proof)?;

        let (scope, repos) = self.begin().await?;
        Self::consume_proof_challenge(&repos, &proof).await?;
        if !repos.key_identity_repo.lock_user(user_id).await? {
            return Err(ServiceError::NotFound);
        }
        let user = repos
            .user_repo
            .get_by_id(user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if user.principal_id.is_some() {
            return Err(ServiceError::Conflict(
                "this account already has a key identity".to_string(),
            ));
        }
        Self::ensure_principal_unused(&repos, &state.principal_id).await?;
        repos
            .key_identity_repo
            .enroll(
                user_id,
                &state.principal_id,
                &state.identity_key.to_string(),
                entries,
            )
            .await?;
        let device = insert_device(&repos, new_device(user_id, &cert, &cmd.device)).await?;
        repos
            .session_repo
            .bind_device(cmd.session_id, device.id)
            .await?;
        Self::commit(scope).await?;

        info!(
            target = "audit",
            event = "key_identity_enrolled",
            user_id = %user_id,
            principal_id = %state.principal_id,
            device_id = %device.device_id,
            "key identity enrolled; password sign-in disabled for this account"
        );
        self.get_my_key_identity(GetMyKeyIdentityQuery { actor: cmd.actor })
            .await
    }

    #[instrument(name = "key_auth.signup_with_key", skip_all, fields(username = %cmd.username))]
    async fn signup_with_key(&self, cmd: SignupWithKeyCmd) -> ServiceResult<SignupResultDto> {
        // The same mode rules as `signupWithInvite`, checked here too so the service never
        // relies on the resolver alone.
        match (&cmd.registration_mode, &cmd.token) {
            (RegistrationMode::InviteOnly, None) => {
                return Err(ServiceError::Validation(
                    "exactly one token is required in invite-only mode".to_string(),
                ));
            }
            (RegistrationMode::Open, Some(_)) => {
                return Err(ServiceError::Validation(
                    "tokens must not be supplied in open registration mode".to_string(),
                ));
            }
            (RegistrationMode::Open, None) => {
                signup::check_open_signup_rate(&self.signup_rate_limiter, &cmd.remote_ip)?;
            }
            (RegistrationMode::InviteOnly, Some(_)) => {}
        }
        let username = cmd.username.as_str();
        signup::validate_username(username)?;
        let display_name = signup::normalize_display_name(cmd.display_name.as_deref(), username)?;

        let token = match &cmd.token {
            Some(token) => Some(signup::check_token(&self.repos.signup, token).await?),
            None => None,
        };
        let PresentedIdentity {
            state,
            cert,
            proof,
            entries,
        } = self.verify_presented(&cmd.key_log, &cmd.device, &cmd.proof)?;

        let (scope, repos) = self.begin().await?;
        Self::consume_proof_challenge(&repos, &proof).await?;
        // Checked for a clear answer; two signups racing for one principal are settled by the
        // unique index, which turns the second insert into `Conflict` as well.
        Self::ensure_principal_unused(&repos, &state.principal_id).await?;
        let user = signup::create_user(
            repos.user_repo.as_ref(),
            NewUser {
                username: username.to_string(),
                password: None,
                email: None,
                display_name: Some(display_name),
                key_identity: Some(NewUserKeyIdentity {
                    principal_id: state.principal_id.clone(),
                    identity_key: state.identity_key.to_string(),
                }),
            },
        )
        .await?;
        repos
            .key_identity_repo
            .insert_key_log(user.id, entries)
            .await?;
        let device = insert_device(&repos, new_device(user.id, &cert, &cmd.device)).await?;
        let server_id = match &token {
            Some(token) => signup::claim_token(&repos.signup, token, user.id).await?,
            None => None,
        };
        let (session, session_token, csrf_token) = signup::create_session(
            repos.session_repo.as_ref(),
            self.session_ttl,
            user.id,
            None,
            None,
        )
        .await?;
        repos
            .session_repo
            .bind_device(session.id, device.id)
            .await?;
        repos.key_identity_repo.touch_device(device.id).await?;
        Self::commit(scope).await?;

        match &token {
            Some(token) => signup::announce_claimed(token, user.id, username, &self.observers),
            None => info!(
                target = "audit",
                event = "signup_open_success",
                user_id = %user.id,
                remote_ip = %cmd.remote_ip,
                credential = "key",
                "user created via open signup"
            ),
        }
        info!(
            target = "audit",
            event = "key_identity_enrolled",
            user_id = %user.id,
            principal_id = %state.principal_id,
            device_id = %device.device_id,
            session_id = %session.id,
            "key-only account created; it signs in with a device key only"
        );
        Ok(SignupResultDto {
            user_id: user.id,
            server_id,
            session: AuthSessionDto {
                user_id: user.id,
                session_id: session.id,
                expires_at: session.expires_at,
            },
            session_token,
            csrf_token,
        })
    }

    #[instrument(name = "key_auth.add_device", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn add_device(&self, cmd: AddDeviceKeyCmd) -> ServiceResult<DeviceKeyDto> {
        let user_id = cmd.actor.user_id;
        let (scope, repos) = self.begin().await?;
        let state = Self::lock_and_load(&repos, user_id).await?;
        let cert = verify_device_cert(&state, &to_statement(&cmd.device)).map_err(invalid)?;
        let device = insert_device(&repos, new_device(user_id, &cert, &cmd.device)).await?;
        Self::commit(scope).await?;
        info!(
            target = "audit",
            event = "device_key_added",
            user_id = %user_id,
            device_id = %device.device_id,
            "device key added"
        );
        Ok(device_dto(device))
    }

    #[instrument(name = "key_auth.revoke_device", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn revoke_device(&self, cmd: RevokeDeviceKeyCmd) -> ServiceResult<DeviceKeyDto> {
        let user_id = cmd.actor.user_id;
        let unverified =
            UnverifiedRevocation::read(&to_statement(&cmd.revocation)).map_err(invalid)?;
        let (scope, repos) = self.begin().await?;
        let state = Self::lock_and_load(&repos, user_id).await?;
        let device = repos
            .key_identity_repo
            .get_device(user_id, unverified.device_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let device_key = PublicKey::parse(&device.public_key, "deviceKey")
            .map_err(|e| ServiceError::internal(anyhow::anyhow!("stored device key: {e}")))?;
        let revocation = unverified.verify(&state, &device_key).map_err(invalid)?;
        let newly_revoked = repos
            .key_identity_repo
            .revoke_device(
                device.id,
                &cmd.revocation.payload,
                &cmd.revocation.signature,
            )
            .await?;
        let ended_sessions = repos.session_repo.revoke_all_for_device(device.id).await?;
        Self::commit(scope).await?;
        if newly_revoked {
            self.devices_changed(user_id).await;
            info!(
                target = "audit",
                event = "device_key_revoked",
                user_id = %user_id,
                device_id = %device.device_id,
                signer = ?revocation.signer,
                ended_sessions,
                "device key revoked"
            );
        }
        let device = self
            .repos
            .key_identity_repo
            .get_device(user_id, device.device_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        Ok(device_dto(device))
    }

    #[instrument(name = "key_auth.rotate", skip_all)]
    async fn rotate(&self, cmd: RotateIdentityKeyCmd) -> ServiceResult<KeyIdentityDto> {
        let entry = to_statement(&cmd.entry);
        let principal_id = rotation_principal_id(&entry).map_err(invalid)?;
        if cmd.actor.is_none() {
            // Unauthenticated recovery: every attempt counts, so the endpoint can't be used to
            // probe principals or grind signatures.
            self.reserve_failure(&cmd.remote_ip)?;
        }
        let user_id = self
            .repos
            .key_identity_repo
            .find_user_by_principal_id(&principal_id)
            .await?
            .ok_or(if cmd.actor.is_some() {
                ServiceError::NotFound
            } else {
                ServiceError::Unauthenticated
            })?;
        if let Some(actor) = &cmd.actor
            && actor.user_id != user_id
        {
            return Err(ServiceError::PermissionDenied);
        }

        let (scope, repos) = self.begin().await?;
        let state = Self::lock_and_load(&repos, user_id).await?;
        let (next, signer) = state.apply(&entry).map_err(invalid)?;
        if signer == RotationSigner::Identity && cmd.actor.is_none() {
            return Err(ServiceError::Unauthenticated);
        }
        // A recovery still goes through at the cap: locking someone out of their account is
        // worse than a log too long to carry to another server.
        if signer == RotationSigner::Identity && next.seq as usize >= MAX_KEY_LOG_LEN {
            return Err(ServiceError::Validation(format!(
                "a key log may have at most {MAX_KEY_LOG_LEN} entries"
            )));
        }
        let cert = match &cmd.device {
            Some(device) => {
                Some(verify_device_cert(&next, &to_statement(device)).map_err(invalid)?)
            }
            None if signer == RotationSigner::Recovery => {
                return Err(ServiceError::Validation(
                    "a recovery rotation needs a device certificate under the new identity key"
                        .to_string(),
                ));
            }
            None => None,
        };

        repos
            .key_identity_repo
            .append_rotation(
                user_id,
                NewKeyLogEntry {
                    seq: next.seq,
                    entry_type: KeyLogEntryType::Rotate,
                    payload: cmd.entry.payload.clone(),
                    signature: cmd.entry.signature.clone(),
                },
                &next.identity_key.to_string(),
            )
            .await
            .map_err(|e| match e.kind() {
                RepoErrorKind::Conflict => {
                    ServiceError::Conflict("the key log moved on; fetch it and retry".to_string())
                }
                _ => e.into(),
            })?;
        let (revoked_devices, ended_sessions) = if signer == RotationSigner::Recovery {
            (
                repos.key_identity_repo.revoke_all_devices(user_id).await?,
                repos.session_repo.revoke_all_for_user(user_id).await?,
            )
        } else {
            (0, 0)
        };
        if let (Some(cert), Some(device)) = (&cert, &cmd.device) {
            insert_device(&repos, new_device(user_id, cert, device)).await?;
        }
        Self::commit(scope).await?;
        if revoked_devices > 0 {
            self.devices_changed(user_id).await;
        }

        info!(
            target = "audit",
            event = "identity_key_rotated",
            user_id = %user_id,
            principal_id = %principal_id,
            seq = next.seq,
            signer = ?signer,
            revoked_devices,
            ended_sessions,
            "identity key rotated"
        );
        self.my_key_identity(user_id).await
    }

    #[instrument(name = "key_auth.publish_profile_card", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn publish_profile_card(
        &self,
        cmd: PublishProfileCardCmd,
    ) -> ServiceResult<ProfileCardDto> {
        let user_id = cmd.actor.user_id;
        let (scope, repos) = self.begin().await?;
        let state = Self::lock_and_load(&repos, user_id).await?;
        let card = verify_profile_card(&state, &to_statement(&cmd.card)).map_err(invalid)?;
        let version = u32::try_from(card.version)
            .map_err(|_| ServiceError::Validation("card version too large".to_string()))?;
        if !repos
            .key_identity_repo
            .upsert_profile_card(user_id, version, &cmd.card.payload, &cmd.card.signature)
            .await?
        {
            return Err(ServiceError::Conflict(
                "a card with this version or a newer one is already published".to_string(),
            ));
        }
        Self::commit(scope).await?;
        let stored = self
            .repos
            .key_identity_repo
            .get_profile_card(user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        Ok(card_dto(stored))
    }

    #[instrument(name = "key_auth.get_my_key_identity", skip_all, fields(actor_id = %query.actor.user_id))]
    async fn get_my_key_identity(
        &self,
        query: GetMyKeyIdentityQuery,
    ) -> ServiceResult<KeyIdentityDto> {
        self.my_key_identity(query.actor.user_id).await
    }

    #[instrument(name = "key_auth.get_key_log", skip_all)]
    async fn get_key_log(&self, query: GetKeyLogQuery) -> ServiceResult<Vec<KeyLogEntryDto>> {
        // A principal id is 49 characters; anything much longer is not one, and is not looked up.
        if query.principal_id.len() > MAX_PRINCIPAL_ID_LEN {
            return Ok(Vec::new());
        }
        let Some(user_id) = self
            .repos
            .key_identity_repo
            .find_user_by_principal_id(&query.principal_id)
            .await?
        else {
            return Ok(Vec::new());
        };
        Ok(self
            .repos
            .key_identity_repo
            .list_key_log(user_id)
            .await?
            .into_iter()
            .map(log_entry_dto)
            .collect())
    }
}

impl DefaultKeyAuthService {
    async fn my_key_identity(&self, user_id: UserId) -> ServiceResult<KeyIdentityDto> {
        let user = self
            .repos
            .user_repo
            .get_by_id(user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let Some(principal_id) = user.principal_id else {
            return Ok(KeyIdentityDto::default());
        };
        let key_log: Vec<KeyLogEntryDto> = self
            .repos
            .key_identity_repo
            .list_key_log(user_id)
            .await?
            .into_iter()
            .map(log_entry_dto)
            .collect();
        let devices = self
            .repos
            .key_identity_repo
            .list_devices(user_id)
            .await?
            .into_iter()
            .map(device_dto)
            .collect();
        let profile_card = self
            .repos
            .key_identity_repo
            .get_profile_card(user_id)
            .await?
            .map(card_dto);
        Ok(KeyIdentityDto {
            principal_id: Some(principal_id),
            key_seq: key_log.last().map(|e| e.seq),
            key_log,
            devices,
            profile_card,
        })
    }
}
