//! Key identity and challenge login. Statement verification lives in `rim_identity`; this module
//! decides who may submit what, stores what verified, and issues sessions.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use sea_orm::DatabaseConnection;
use tracing::{info, instrument, warn};

use rim_auth::csrf::generate_csrf_token;
use rim_auth::session_token::{generate_challenge_nonce, generate_session_token, hash_token};
use rim_core::account_password::repository::AccountPasswordRepository;
use rim_core::auth::model::NewSession;
use rim_core::auth::repository::SessionRepository;
use rim_core::host::model::UserKind;
use rim_core::host::repository::HostRepository;
use rim_core::key_identity::model::{
    DeviceKey, DeviceKeyId, KeyChallengePurpose, KeyLogEntry, KeyLogEntryType, NewDeviceKey,
    NewKeyLogEntry, StoredProfileCard,
};
use rim_core::key_identity::repository::{KeyChallengeRepository, KeyIdentityRepository};
use rim_core::relay::repository::RelayAbuseRepository;
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
use rim_services::auth::terms::{accepted_terms, confirmed_minimum_age};
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::events::Observers;
use rim_services::host::dto::IdentityRefresh;
use rim_services::host::service::HostService;
use rim_services::key_auth::command::{
    AddDeviceKeyCmd, EnrollIdentityKeyCmd, GetKeyLogQuery, GetMyKeyIdentityQuery,
    IssueKeyChallengeCmd, LoginWithKeyCmd, PublishProfileCardCmd, RevokeDeviceKeyCmd,
    RotateIdentityKeyCmd, SignedPayload, SignupWithKeyCmd,
};
use rim_services::key_auth::dto::{
    DeviceKeyDto, KeyChallengeDto, KeyIdentityDto, KeyLogEntryDto, ProfileCardDto,
};
use rim_services::key_auth::service::KeyAuthService;
use rim_services::limits::InstanceLimits;
use rim_services::relay::service::RelayService;
use rim_services::remote_workspace::service::RevocationFanout;
use rim_services::session_revocation::SessionRevocations;
use rim_services::{ServiceError, ServiceResult};

use crate::account_cap::AccountCap;
use crate::account_password::{registration_record, store_new_password};
use crate::default_workspace;
use crate::password_key::PasswordKey;
use crate::quota::Quotas;
use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::repos::AppRepos;
use crate::reserved_names::ReservedNames;
use crate::signup::{self, SignupRepos};

/// How long a login challenge stays valid.
const CHALLENGE_TTL_SECONDS: i64 = 120;

/// Failure-limiter key shared by key logins and unauthenticated recoveries.
pub(crate) const FAILURE_KEY: &str = "key_auth_failure";

/// How long a session lasts for someone whose relay is another server, signing in directly
/// while their relay is unreachable.
const REMOTE_SESSION_TTL: Duration = Duration::hours(24);

/// How long a direct sign-in waits for the person's relays to refresh their identity.
const REMOTE_REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

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
    /// Whether a user holding a principal is someone from another server, whom a key-only
    /// signup turns into a local user.
    pub host_repo: Arc<dyn HostRepository>,
    /// Principals this server refuses.
    pub relay_abuse_repo: Arc<dyn RelayAbuseRepository>,
    /// The passwords local accounts sign in with besides their device key.
    pub account_password_repo: Arc<dyn AccountPasswordRepository>,
}

/// An audience as compared: a server id (`rims1_…`) as it is, since base64url is
/// case-sensitive; a `host[:port]` lowercased. Host names cannot contain `_`, so the prefix is
/// unambiguous.
pub(crate) fn normalize_audience(audience: &str) -> String {
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
    /// Told when a user's card or devices change, so hosts holding them get the new identity.
    relay_service: Option<Arc<dyn RelayService>>,
    /// This server's id, which a proof-of-work signup is bound to. `None` in unit tests that
    /// do not sign up in `pow` mode.
    server_id: Option<String>,
    /// Devices and card size per user.
    quotas: Quotas,
    /// The cap on workspace members a signup with a workspace invite joins under.
    limits: InstanceLimits,
    /// Refreshes a remote user's identity from their relays before they sign in directly.
    host_service: Option<Arc<dyn HostService>>,
    /// Sends a revoked device's revocation to every host holding its person.
    revocation_fanout: Option<Arc<dyn RevocationFanout>>,
    /// Usernames and display names signups may not take.
    reserved_names: ReservedNames,
    /// Told of the sessions a device revocation or a recovery ends, so their live connections
    /// close.
    session_revocations: Arc<dyn SessionRevocations>,
    /// The terms of service URL every signup must accept; `None` when there are none.
    terms_url: Option<String>,
    /// The minimum age every signup must confirm; `None` when there is none.
    minimum_age: Option<u8>,
    /// The workspace every new account joins as a Member, and the definition an owner claim
    /// creates it from.
    default_workspace: default_workspace::DefaultWorkspace,
    /// The cap on local accounts every key-only signup checks.
    account_cap: AccountCap,
    /// Registers the password a signup brings and opens the server share a sign-in returns.
    /// `None` in unit tests that neither sign up nor sign in with a password.
    password_key: Option<PasswordKey>,
}

/// What the password of a key sign-in requires.
enum PasswordCheck {
    /// Someone from another server: their password lives at home.
    NotNeeded,
    /// A local account without a password: it signs in and must set one.
    MustSet,
    /// A local account with a password: the ticket with this token hash must be consumed.
    Ticket(String),
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
    crate::util::statement_refused(err)
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
            relay_service: None,
            server_id: None,
            quotas: Quotas::default(),
            limits: InstanceLimits::default(),
            host_service: None,
            revocation_fanout: None,
            reserved_names: ReservedNames::default(),
            session_revocations: crate::session_revocation::unshared(),
            terms_url: None,
            minimum_age: None,
            default_workspace: default_workspace::DefaultWorkspace::None,
            account_cap: AccountCap::default(),
            password_key: None,
        }
    }

    /// Registers signup passwords and opens server shares with `key`.
    pub fn with_password_key(mut self, key: Option<PasswordKey>) -> Self {
        self.password_key = key;
        self
    }

    /// The per-IP limiter failed key sign-ins count against, which the password login start
    /// shares.
    pub(crate) fn failure_rate_limiter(&self) -> RegistrationRateLimiter {
        self.failure_rate_limiter.clone()
    }

    fn password_key(&self) -> ServiceResult<&PasswordKey> {
        self.password_key.as_ref().ok_or_else(|| {
            ServiceError::internal(anyhow::anyhow!("the password key is not loaded"))
        })
    }

    /// What the password of a key sign-in requires. An account of this server with a password
    /// must bring a ticket from `passwordLoginFinish`, consumed inside the session transaction;
    /// one without a password signs in with its key alone and must set one. Accounts of other
    /// servers keep their password at home.
    ///
    /// # Errors
    /// `Unauthenticated` when the account has a password and no ticket came.
    async fn check_password(
        &self,
        user_id: UserId,
        remote: bool,
        ticket: Option<&str>,
    ) -> ServiceResult<PasswordCheck> {
        if remote {
            return Ok(PasswordCheck::NotNeeded);
        }
        if self
            .repos
            .account_password_repo
            .get(user_id)
            .await?
            .is_none()
        {
            return Ok(PasswordCheck::MustSet);
        }
        match ticket {
            Some(ticket) => Ok(PasswordCheck::Ticket(hash_token(ticket))),
            None => {
                info!(
                    target: "audit",
                    event = "key_login_failure",
                    reason = "password_missing",
                    user_id = %user_id,
                    "key login refused: the account has a password and no ticket came"
                );
                Err(ServiceError::Unauthenticated)
            }
        }
    }

    /// A key sign-in after the failure budget is charged; see
    /// [`KeyAuthService::login_with_key`].
    async fn sign_in(&self, cmd: &LoginWithKeyCmd) -> ServiceResult<LoginResultDto> {
        let remote = self.refresh_if_remote(&cmd.proof).await?;
        let (user_id, device) = self.verify_login(&cmd.proof).await?;
        let check = self
            .check_password(user_id, remote, cmd.password_ticket.as_deref())
            .await?;
        let must_set_password = matches!(check, PasswordCheck::MustSet);

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
                target: "audit",
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
                expires_at: Utc::now()
                    + if remote {
                        self.session_ttl.min(REMOTE_SESSION_TTL)
                    } else {
                        self.session_ttl
                    },
                user_agent: None,
                ip: None,
            })
            .await?;
        repos
            .session_repo
            .bind_device(session.id, device.id)
            .await?;
        let keys = crate::session_key::keep_new_session(
            repos.session_repo.as_ref(),
            self.password_key.as_ref(),
            session.id,
            cmd.keep,
        )
        .await?;
        repos.key_identity_repo.touch_device(device.id).await?;
        match &check {
            PasswordCheck::Ticket(token_hash) => {
                if !repos
                    .account_password_repo
                    .consume_ticket(token_hash, user_id, Utc::now())
                    .await?
                {
                    info!(
                        target: "audit",
                        event = "key_login_failure",
                        reason = "password_ticket_invalid",
                        user_id = %user_id,
                        "key login refused: the password ticket is unknown, used, expired or another account's"
                    );
                    return Err(ServiceError::Unauthenticated);
                }
            }
            PasswordCheck::MustSet => repos.session_repo.require_password(session.id).await?,
            PasswordCheck::NotNeeded => {}
        }
        Self::commit(scope).await?;

        info!(
            target: "audit",
            event = "key_login_success",
            remote,
            password = matches!(check, PasswordCheck::Ticket(_)),
            must_set_password,
            kept = keys.kept,
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
            must_set_password,
            keys,
        })
    }

    /// The server id proof-of-work signups are bound to.
    pub fn with_server_id(mut self, server_id: String) -> Self {
        self.server_id = Some(server_id);
        self
    }

    pub fn with_quotas(mut self, quotas: Quotas) -> Self {
        self.quotas = quotas;
        self
    }

    /// Applies the instance's caps: workspace members and, with a cap of its own that
    /// `with_account_cap` can replace by a shared one, local accounts.
    pub fn with_limits(mut self, limits: InstanceLimits) -> Self {
        self.limits = limits;
        self.account_cap = AccountCap::new(limits.accounts);
        self
    }

    /// Checks every key-only signup against `cap`, built from the same limits and shared with
    /// the password signups, so a signup on either path refreshes the other's "full" answer.
    pub(crate) fn with_account_cap(mut self, cap: AccountCap) -> Self {
        self.account_cap = cap;
        self
    }

    pub fn with_reserved_names(mut self, reserved_names: ReservedNames) -> Self {
        self.reserved_names = reserved_names;
        self
    }

    /// Requires every signup to accept the terms at `terms_url`.
    pub fn with_terms_url(mut self, terms_url: Option<String>) -> Self {
        self.terms_url = terms_url;
        self
    }

    /// Requires every signup to confirm the person has reached `minimum_age`.
    pub fn with_minimum_age(mut self, minimum_age: Option<u8>) -> Self {
        self.minimum_age = minimum_age;
        self
    }

    /// Makes every new account a Member of `workspace`: a workspace id, or a definition an owner
    /// claim creates the workspace from.
    pub fn with_default_workspace(
        mut self,
        workspace: impl Into<default_workspace::DefaultWorkspace>,
    ) -> Self {
        self.default_workspace = workspace.into();
        self
    }

    /// `Forbidden` when an operator blocked `principal_id` on this server, with an audit
    /// event naming `action`.
    async fn ensure_not_blocked(&self, principal_id: &str, action: &str) -> ServiceResult<()> {
        if self
            .repos
            .relay_abuse_repo
            .is_principal_blocked(principal_id)
            .await?
        {
            info!(
                target: "audit",
                event = "relay.blocked_principal_refused",
                principal_id = %principal_id,
                action,
                "refused a principal blocked on this server"
            );
            return Err(ServiceError::Forbidden(
                "this key identity is blocked on this server".to_string(),
            ));
        }
        Ok(())
    }

    /// Check a key-only signup's proof-of-work: its hash over this server's id, the log's
    /// inception and the nonce has `bits` leading zero bits.
    fn check_proof_of_work(
        &self,
        key_log: &[SignedPayload],
        nonce: &str,
        bits: u32,
    ) -> ServiceResult<()> {
        let server_id = self.server_id.as_deref().ok_or_else(|| {
            ServiceError::internal(anyhow::anyhow!(
                "proof-of-work registration needs this server's id"
            ))
        })?;
        let inception = key_log
            .first()
            .ok_or_else(|| ServiceError::Validation("the key log is empty".to_string()))?;
        rim_s2s::pow::check_pow(server_id, &inception.payload, nonce, bits).map_err(|e| {
            info!(reason = %e, pow_bits = bits, "refused a signup's proof-of-work");
            crate::metrics::pow_rejected();
            ServiceError::Validation(e.to_string())
        })
    }

    pub fn with_host_service(mut self, host_service: Arc<dyn HostService>) -> Self {
        self.host_service = Some(host_service);
        self
    }

    pub fn with_revocation_fanout(mut self, fanout: Arc<dyn RevocationFanout>) -> Self {
        self.revocation_fanout = Some(fanout);
        self
    }

    /// Report the sessions a device revocation or a recovery ends to `revocations`.
    pub fn with_session_revocations(mut self, revocations: Arc<dyn SessionRevocations>) -> Self {
        self.session_revocations = revocations;
        self
    }

    /// Start sending a revocation to the hosts holding the user, in the background.
    fn revocation_committed(&self, user_id: UserId, device_key_id: DeviceKeyId) {
        if let Some(fanout) = &self.revocation_fanout {
            let fanout = Arc::clone(fanout);
            tokio::spawn(async move {
                if let Err(e) = fanout.revocation_committed(user_id, device_key_id).await {
                    warn!(error = %e, user_id = %user_id, "could not send a revocation to hosts");
                }
            });
        }
    }

    /// Before a person from another server signs in directly, refetch their identity from
    /// their relays, so a device revoked there is revoked here; relays that do not answer in
    /// time leave the stored copy. Returns whether the principal is held here as remote.
    ///
    /// # Errors
    /// `Unauthenticated` when a relay answered with an identity this server refused and no
    /// relay's was stored.
    async fn refresh_if_remote(&self, proof: &SignedPayload) -> ServiceResult<bool> {
        let Ok(unverified) = UnverifiedLogin::read(&to_statement(proof)) else {
            return Ok(false);
        };
        let Some(user_id) = self
            .repos
            .key_identity_repo
            .find_user_by_principal_id(&unverified.principal_id)
            .await?
        else {
            return Ok(false);
        };
        if self.repos.host_repo.user_kind(user_id).await? != Some(UserKind::Remote) {
            return Ok(false);
        }
        if let Some(host) = &self.host_service {
            match tokio::time::timeout(
                REMOTE_REFRESH_TIMEOUT,
                host.refresh_identity(unverified.principal_id.clone(), None),
            )
            .await
            {
                Ok(Ok(IdentityRefresh::Applied(_))) => {}
                Ok(Ok(IdentityRefresh::Refused)) => {
                    info!(
                        target: "audit",
                        event = "key_login_failure",
                        reason = "identity_refused",
                        user_id = %user_id,
                        "key login refused: the person's relays served an identity this server refused"
                    );
                    return Err(ServiceError::Unauthenticated);
                }
                Ok(Ok(IdentityRefresh::Unreachable)) => {
                    warn!(user_id = %user_id, "the relays did not answer before a direct sign-in; using the stored identity")
                }
                Ok(Err(e)) => {
                    warn!(error = %e, user_id = %user_id, "could not refresh a remote identity before a direct sign-in")
                }
                Err(_) => {
                    warn!(user_id = %user_id, "the relays did not answer before a direct sign-in; using the stored identity")
                }
            }
        }
        Ok(true)
    }

    pub fn with_relay_service(mut self, relay_service: Arc<dyn RelayService>) -> Self {
        self.relay_service = Some(relay_service);
        self
    }

    /// Push the user's identity to the hosts that know them, in the background.
    fn identity_changed(&self, user_id: UserId) {
        if let Some(relay) = &self.relay_service {
            let relay = Arc::clone(relay);
            tokio::spawn(async move {
                if let Err(e) = relay.identity_changed(user_id).await {
                    warn!(error = %e, user_id = %user_id, "could not push an identity change to hosts");
                }
            });
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
            } => {
                crate::metrics::rate_limited(FAILURE_KEY);
                Err(ServiceError::RateLimited {
                    retry_after_seconds,
                })
            }
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
            .consume(
                proof.challenge_id.into(),
                &proof.nonce,
                &KeyChallengePurpose::Login,
            )
            .await?
        {
            return Err(ServiceError::Validation(
                "the challenge is unknown, already used or expired".to_string(),
            ));
        }
        Ok(())
    }

    /// `Conflict` when an account on this server already has `principal_id`; `Validation` when
    /// a deleted account had it.
    async fn ensure_principal_unused(
        repos: &KeyAuthRepos,
        principal_id: &str,
    ) -> ServiceResult<()> {
        Self::ensure_principal_not_retired(repos, principal_id).await?;
        match Self::remote_holder(repos, principal_id).await? {
            None => Ok(()),
            Some(_) => Err(Self::principal_taken()),
        }
    }

    /// `Validation` when an account deleted on this server held `principal_id`; its tombstone
    /// keeps the id, so the key identity cannot come back here.
    async fn ensure_principal_not_retired(
        repos: &KeyAuthRepos,
        principal_id: &str,
    ) -> ServiceResult<()> {
        if repos
            .key_identity_repo
            .principal_retired(principal_id)
            .await?
        {
            return Err(ServiceError::Validation(
                "this key identity belonged to an account deleted on this server and cannot be                  used here again; create a new identity"
                    .to_string(),
            ));
        }
        Ok(())
    }

    fn principal_taken() -> ServiceError {
        ServiceError::Conflict(
            "another account on this server already uses this key identity".to_string(),
        )
    }

    /// The user holding `principal_id`: `Some(Some(id))` for someone from another server,
    /// `Some(None)` for a local account, `None` for nobody.
    async fn remote_holder(
        repos: &KeyAuthRepos,
        principal_id: &str,
    ) -> ServiceResult<Option<Option<UserId>>> {
        let Some(user_id) = repos
            .key_identity_repo
            .find_user_by_principal_id(principal_id)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(
            (repos.host_repo.user_kind(user_id).await? == Some(UserKind::Remote))
                .then_some(user_id),
        ))
    }

    /// Turn a remote user into the local account a key-only signup creates: their stored log
    /// must be a prefix of the presented one, which extends it; the presented device is reused
    /// when it is stored already. Memberships stay. `email` is the address an email-bound
    /// invite gives the account, checked as a new account's is.
    #[allow(clippy::too_many_arguments)]
    async fn adopt_remote(
        repos: &KeyAuthRepos,
        user_id: UserId,
        username: &str,
        display_name: &str,
        email: Option<&str>,
        state: &KeyState,
        entries: Vec<NewKeyLogEntry>,
        cert: &DeviceCert,
        device: &SignedPayload,
    ) -> ServiceResult<DeviceKey> {
        if !repos.key_identity_repo.lock_user(user_id).await? {
            return Err(ServiceError::NotFound);
        }
        let stored = repos.key_identity_repo.list_key_log(user_id).await?;
        let prefix = stored.len() <= entries.len()
            && stored
                .iter()
                .zip(&entries)
                .all(|(s, e)| s.payload == e.payload && s.signature == e.signature);
        if !prefix {
            return Err(Self::principal_taken());
        }
        let from = stored.len();
        for entry in entries.into_iter().skip(from) {
            repos
                .key_identity_repo
                .append_rotation(user_id, entry, &state.identity_key.to_string())
                .await?;
        }
        if let Some(email) = email {
            signup::check_signup_email(repos.user_repo.as_ref(), email, username).await?;
        }
        repos
            .host_repo
            .make_local(user_id, username, display_name, email)
            .await
            .map_err(|e| match e.kind() {
                RepoErrorKind::Conflict => {
                    ServiceError::Conflict("this username is taken".to_string())
                }
                _ => e.into(),
            })?;
        match repos
            .key_identity_repo
            .get_device(user_id, cert.device_id)
            .await?
        {
            Some(existing)
                if existing.revoked_at.is_none()
                    && existing.public_key == cert.device_key.to_string() =>
            {
                Ok(existing)
            }
            Some(_) => Err(ServiceError::Conflict(
                "this device id or device key is already enrolled or was revoked".to_string(),
            )),
            None => insert_device(repos, new_device(user_id, cert, device)).await,
        }
    }

    /// Verify a login proof and claim its challenge. Every refusal is `Unauthenticated` except
    /// a valid signature from a revoked device.
    async fn verify_login(&self, proof: &SignedPayload) -> ServiceResult<(UserId, DeviceKey)> {
        let fail = |reason: &str| {
            info!(
                target: "audit",
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
            .consume(
                login.challenge_id.into(),
                &login.nonce,
                &KeyChallengePurpose::Login,
            )
            .await?
        {
            return Err(fail("challenge_invalid"));
        }
        if device.revoked_at.is_some() {
            info!(
                target: "audit",
                event = "key_login_failure",
                reason = "device_revoked",
                user_id = %user_id,
                device_id = %device.device_id,
                "key login refused: device revoked"
            );
            return Err(ServiceError::PermissionDenied);
        }
        self.ensure_not_blocked(&login.principal_id, "login")
            .await?;
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
            crate::metrics::rate_limited("key_challenge");
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
            .create(
                &nonce,
                now + Duration::seconds(CHALLENGE_TTL_SECONDS),
                &KeyChallengePurpose::Login,
            )
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
        let result = self.sign_in(&cmd).await;
        if !matches!(
            result,
            Err(ServiceError::Unauthenticated | ServiceError::PermissionDenied)
        ) {
            self.failure_rate_limiter
                .refund(&cmd.remote_ip, FAILURE_KEY, reserved_at);
        }
        result
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
        self.ensure_not_blocked(&state.principal_id, "enroll")
            .await?;

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
        // No other session is bound to a device yet, so none of them may outlive the password.
        let ended_sessions = repos
            .session_repo
            .revoke_all_for_user_except(user_id, cmd.session_id)
            .await?;
        Self::commit(scope).await?;
        self.session_revocations
            .revoke_user_except(user_id, cmd.session_id);

        info!(
            target: "audit",
            event = "key_identity_enrolled",
            user_id = %user_id,
            principal_id = %state.principal_id,
            device_id = %device.device_id,
            ended_sessions,
            "key identity enrolled; password sign-in disabled and other sessions ended"
        );
        self.get_my_key_identity(GetMyKeyIdentityQuery { actor: cmd.actor })
            .await
    }

    #[instrument(name = "key_auth.signup_with_key", skip_all, fields(username = %cmd.username))]
    async fn signup_with_key(&self, cmd: SignupWithKeyCmd) -> ServiceResult<SignupResultDto> {
        self.account_cap
            .check(self.repos.user_repo.as_ref())
            .await?;
        let terms_accepted_url =
            accepted_terms(self.terms_url.as_deref(), cmd.terms_accepted_url.as_deref())?;
        let confirmed_age = confirmed_minimum_age(self.minimum_age, cmd.confirmed_minimum_age)?;
        // The same mode rules as `signupWithInvite`, checked here too so the service never
        // relies on the resolver alone. In pow mode a proof-of-work stands in for the token.
        let refuse = |why: &str| Err(ServiceError::Validation(why.to_string()));
        let pow_bits = match (&cmd.registration_mode, &cmd.token, &cmd.proof_of_work) {
            (RegistrationMode::Pow { .. }, Some(_), Some(_)) => {
                return refuse("give a registration token or proofOfWork, not both");
            }
            (RegistrationMode::Pow { .. }, None, None) => {
                return refuse("a registration token or proofOfWork is required");
            }
            (RegistrationMode::Pow { bits }, None, Some(nonce)) => {
                signup::check_open_signup_rate(&self.signup_rate_limiter, &cmd.remote_ip)?;
                self.check_proof_of_work(&cmd.key_log, nonce, *bits)?;
                Some(*bits)
            }
            (RegistrationMode::Pow { .. }, Some(_), None) => None,
            (_, _, Some(_)) => {
                return refuse("proofOfWork is only taken in pow registration mode");
            }
            (RegistrationMode::InviteOnly, None, None) => {
                return refuse("exactly one token is required in invite-only mode");
            }
            (RegistrationMode::Open, Some(_), None) => {
                return refuse("tokens must not be supplied in open registration mode");
            }
            (RegistrationMode::Open, None, None) => {
                signup::check_open_signup_rate(&self.signup_rate_limiter, &cmd.remote_ip)?;
                None
            }
            (RegistrationMode::InviteOnly, Some(_), None) => None,
        };
        let normalized_username = signup::normalize_username(&cmd.username)?;
        let username = normalized_username.as_str();
        // The display name falls back to the username as typed, keeping its case.
        let display_name =
            signup::normalize_display_name(cmd.display_name.as_deref(), cmd.username.trim())?;
        signup::check_reserved(&self.reserved_names, username, &display_name)?;

        let token = match &cmd.token {
            Some(token) => Some(signup::check_token(&self.repos.signup, token, None).await?),
            None => None,
        };
        // An email-bound instance invite gives the account its address.
        let email = match &token {
            Some(signup::CheckedToken::InstanceInvite(invite)) => {
                invite.email.as_deref().map(|e| e.trim().to_lowercase())
            }
            _ => None,
        };
        let PresentedIdentity {
            state,
            cert,
            proof,
            entries,
        } = self.verify_presented(&cmd.key_log, &cmd.device, &cmd.proof)?;
        self.ensure_not_blocked(&state.principal_id, "signup")
            .await?;
        let password_record = registration_record(&cmd.password_registration)?;
        let password_key = self.password_key()?;

        let (scope, repos) = self.begin().await?;
        Self::consume_proof_challenge(&repos, &proof).await?;
        Self::ensure_principal_not_retired(&repos, &state.principal_id).await?;
        // A remote user signing up here becomes local too, so both branches below count.
        self.account_cap.admit(repos.user_repo.as_ref()).await?;
        // Checked for a clear answer; two signups racing for one principal are settled by the
        // unique index, which turns the second insert into `Conflict` as well.
        let (user_id, device) = match Self::remote_holder(&repos, &state.principal_id).await? {
            Some(None) => return Err(Self::principal_taken()),
            Some(Some(remote)) => {
                let device = Self::adopt_remote(
                    &repos,
                    remote,
                    username,
                    &display_name,
                    email.as_deref(),
                    &state,
                    entries,
                    &cert,
                    &cmd.device,
                )
                .await?;
                info!(
                    target: "audit",
                    event = "key_identity_adopted",
                    user_id = %remote,
                    principal_id = %state.principal_id,
                    "a remote user signed up here and is now local"
                );
                (remote, device)
            }
            None => {
                let user = signup::create_user(
                    repos.user_repo.as_ref(),
                    NewUser {
                        username: username.to_string(),
                        password: None,
                        email,
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
                (user.id, device)
            }
        };
        let user = repos
            .user_repo
            .get_by_id(user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        signup::record_terms(
            repos.user_repo.as_ref(),
            user.id,
            terms_accepted_url,
            confirmed_age,
        )
        .await?;
        let server_id = match &token {
            Some(token) => {
                signup::claim_token(
                    &repos.signup,
                    token,
                    user.id,
                    self.limits.members_per_workspace,
                )
                .await?
            }
            None => None,
        };
        let claimed = default_workspace::claim(
            &repos.signup,
            &self.default_workspace,
            token.as_ref(),
            user.id,
        )
        .await?;
        let server_id = server_id.or(claimed.as_ref().map(|c| c.workspace_id));
        let joined_default =
            default_workspace::join(&repos.signup, &self.default_workspace, user.id).await?;
        let server_share = store_new_password(
            repos.account_password_repo.as_ref(),
            password_key,
            user.id,
            password_record,
            Utc::now(),
        )
        .await?;
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
        let keys = crate::session_key::keep_new_session(
            repos.session_repo.as_ref(),
            Some(password_key),
            session.id,
            cmd.keep,
        )
        .await?;
        repos.key_identity_repo.touch_device(device.id).await?;
        Self::commit(scope).await?;
        self.account_cap.admitted();

        crate::metrics::registered();
        match (&token, pow_bits) {
            (Some(token), _) => signup::announce_claimed(token, user.id, username, &self.observers),
            (None, Some(bits)) => info!(
                target: "audit",
                event = "relay.registration_pow",
                user_id = %user.id,
                principal_id = %state.principal_id,
                pow_bits = bits,
                remote_ip = %cmd.remote_ip,
                "user created via a proof-of-work signup"
            ),
            (None, None) => info!(
                target: "audit",
                event = "signup_open_success",
                user_id = %user.id,
                remote_ip = %cmd.remote_ip,
                credential = "key",
                "user created via open signup"
            ),
        }
        default_workspace::announce_claim(claimed, user.id, &self.observers);
        default_workspace::announce(joined_default, user.id, username, &self.observers);
        info!(
            target: "audit",
            event = "key_identity_enrolled",
            user_id = %user.id,
            principal_id = %state.principal_id,
            device_id = %device.device_id,
            session_id = %session.id,
            "key account created; it signs in with a device key and its password"
        );
        info!(
            target: "audit",
            event = "password_set",
            user_id = %user.id,
            "password registered at signup"
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
            server_share: Some(server_share.to_vec()),
            keys,
        })
    }

    #[instrument(name = "key_auth.add_device", skip_all, fields(actor_id = %cmd.actor.user_id))]
    async fn add_device(&self, cmd: AddDeviceKeyCmd) -> ServiceResult<DeviceKeyDto> {
        let user_id = cmd.actor.user_id;
        let (scope, repos) = self.begin().await?;
        let state = Self::lock_and_load(&repos, user_id).await?;
        let cert = verify_device_cert(&state, &to_statement(&cmd.device)).map_err(invalid)?;
        let active = repos
            .key_identity_repo
            .list_devices(user_id)
            .await?
            .iter()
            .filter(|d| d.revoked_at.is_none())
            .count();
        if active >= self.quotas.devices_per_user {
            return Err(ServiceError::Validation(format!(
                "this instance allows {} devices per user; revoke one first",
                self.quotas.devices_per_user
            )));
        }
        let device = insert_device(&repos, new_device(user_id, &cert, &cmd.device)).await?;
        Self::commit(scope).await?;
        info!(
            target: "audit",
            event = "device_key_added",
            user_id = %user_id,
            device_id = %device.device_id,
            "device key added"
        );
        self.identity_changed(user_id);
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
        self.session_revocations.revoke_device(device.id);
        if newly_revoked {
            self.devices_changed(user_id).await;
            self.identity_changed(user_id);
            self.revocation_committed(user_id, device.id);
            info!(
                target: "audit",
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
        if signer == RotationSigner::Recovery {
            self.session_revocations.revoke_user(user_id);
        }
        if revoked_devices > 0 {
            self.devices_changed(user_id).await;
        }
        self.identity_changed(user_id);

        info!(
            target: "audit",
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
        if cmd.card.payload.len() > self.quotas.card_max_bytes {
            return Err(ServiceError::Validation(format!(
                "a profile card is at most {} bytes",
                self.quotas.card_max_bytes
            )));
        }
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
        self.identity_changed(user_id);
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
