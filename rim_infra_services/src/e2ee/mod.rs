//! End-to-end encryption on the server: key package storage and claims, the sequencing of each
//! encrypted conversation's MLS group (one Commit per epoch), Welcome delivery, re-add
//! requests, franked reports and retention. The server reads only MLS framing and the
//! authenticated data; it never decrypts.
//!
//! Group membership follows conversation membership. The devices a group should hold are the
//! MLS-active devices of its members; the differences with the group's leaves are the pending
//! adds and removes that member devices commit.

pub(crate) mod aad;
pub(crate) mod franking;
pub(crate) mod openmls_wire;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use sea_orm::DatabaseConnection;
use subtle::ConstantTimeEq;
use tracing::{info, instrument};

use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::e2ee::model::{
    E2eeDevice, MlsCommit, MlsDesiredDevice, MlsDevice, MlsGroup, MlsGroupDevice, MlsKeyPackage,
    MlsKeyPackageStatus, MlsPurged, MlsReaddRequest, MlsRetention, MlsWelcomeBacklog, NewMlsCommit,
    NewMlsGroup, NewMlsKeyPackage, NewMlsReaddRequest, NewMlsWelcome,
};
use rim_core::e2ee::repository::{E2eeRepository, GroupLock};
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::message::repository::MessageRepository;
use rim_core::moderation::model::{NewFrankedReport, NewMessageReport};
use rim_core::moderation::repository::ModerationRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_infra_db::{TransactionScope, begin_transaction};
use rim_services::audit::service::AuditLogService;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::command::verb::TypedVerb;
use rim_services::e2ee::command::{
    AckMlsWelcomesCmd, ClaimMlsKeyPackagesCmd, GetMlsGroupQuery, ListMlsCommitsQuery,
    ListMlsWelcomesQuery, MlsCaller, PrincipalDevicesQuery, ReportEncryptedMessageCmd,
    RequestMlsReaddCmd, SubmitMlsCommitCmd, UploadMlsKeyPackagesCmd,
};
use rim_services::e2ee::dto::{
    ClaimedKeyPackageDto, MlsCommitDto, MlsCommitPageDto, MlsDeviceDto, MlsGroupDto,
    MlsKeyPackageStatusDto, MlsPendingDeviceDto, MlsReaddRequestDto, MlsWelcomeDto,
    PrincipalDeviceDto, PrincipalDevicesDto,
};
use rim_services::e2ee::service::E2eeService;
use rim_services::e2ee::wire::{MlsContentType, MlsWire, PrivateMessageInfo};
use rim_services::e2ee::{
    ACCEPTED_CIPHER_SUITES, MAX_AVAILABLE_KEY_PACKAGES, MAX_COMMIT_BYTES, MAX_CONTENT_BYTES,
    MAX_ENCRYPTED_MEMBERS, MAX_GROUP_DEVICES, MAX_KEY_PACKAGE_BYTES, MAX_KEY_PACKAGES_PER_UPLOAD,
    MAX_PENDING_WELCOME_BYTES, MAX_PENDING_WELCOMES, MAX_WELCOME_BYTES, MLS_ACTIVE_DAYS,
    PREFERRED_CIPHER_SUITE, mls_group_id, parse_mls_group_id,
};
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::host::dto::RemoteClaim;
use rim_services::host::service::RemoteKeyPackages;
use rim_services::{ServiceError, ServiceResult};

use crate::e2ee::aad::{CommitAad, parse_commit};
use crate::e2ee::franking::{FRANKING_LEN, ReceiptFields, commitment, verify_receipt};
use crate::franking_keys::SharedFrankingKeys;
use crate::moderation::{ModerationAuditEntry, record_audit_action};
use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::repos::AppRepos;
use crate::util::{
    ConversationAccessRepos, ensure_conversation_member_access,
    ensure_conversation_writable_with_standing,
};

/// How long a claimed key package may be used by a Commit.
const CLAIM_VALIDITY: Duration = Duration::hours(1);
/// How long a re-add request stays open.
const READD_REQUEST_TTL: Duration = Duration::days(7);
/// How far a re-add request's `issuedAt` may be from the server's clock.
const READD_FRESHNESS: Duration = Duration::minutes(10);
/// How long a re-add request's nonce is kept; longer than a request stays fresh.
const READD_NONCE_TTL: Duration = Duration::days(1);
/// How long a device that asked to be re-added waits before it may start a new incarnation.
const RECOVERY_WAIT: Duration = Duration::minutes(10);
/// How long only the creator's devices may create the group of a conversation created
/// encrypted.
const CREATOR_HEAD_START: Duration = Duration::seconds(60);
/// How long a Welcome waits for its devices.
const WELCOME_TTL: Duration = Duration::days(30);
/// Furthest a key package may expire.
const MAX_KEY_PACKAGE_LIFETIME: Duration = Duration::days(90);
/// Slack on that limit for a client clock ahead of the server's.
const KEY_PACKAGE_CLOCK_MARGIN: Duration = Duration::minutes(5);
/// Most Welcomes one acknowledgement names.
const MAX_ACKS: usize = 100;
/// Most principals one `principalDevices` call names.
const MAX_PRINCIPALS: usize = 100;
/// Longest accepted report reason, in characters.
const MAX_REPORT_REASON_CHARS: usize = 1000;
/// Commit pages: default and largest size.
const DEFAULT_COMMIT_PAGE: i32 = 50;
const MAX_COMMIT_PAGE: i32 = 100;
/// Largest page of pending Welcomes, and the size of a page that names none; one page fits one
/// acknowledgement.
const MAX_WELCOME_PAGE: i32 = 100;

/// Audit command ids.
const AUDIT_ENABLE: &str = "e2ee.enable";
const AUDIT_REINCARNATE: &str = "e2ee.reincarnate";
const AUDIT_REPORT: &str = "e2ee.report";
/// `initiator` of the e2ee audit rows.
pub(crate) const AUDIT_INITIATOR: &str = "e2ee";

/// Limiter keys.
const CLAIM_KEY: &str = "mls_claim";
const ONE_TIME_CLAIM_KEY: &str = "mls_one_time_claim";
const READD_KEY: &str = "mls_readd";

pub(crate) fn not_bound() -> ServiceError {
    ServiceError::Forbidden("this session is not bound to a device".to_string())
}

/// The session's device, or `Forbidden`.
pub(crate) fn require_device(caller: &MlsCaller) -> ServiceResult<DeviceKeyId> {
    caller.device_key_id.ok_or_else(not_bound)
}

/// The session's device row: the caller's own and unrevoked, or `Forbidden`.
pub(crate) async fn session_device(
    e2ee_repo: &dyn E2eeRepository,
    caller: &MlsCaller,
) -> ServiceResult<E2eeDevice> {
    let id = require_device(caller)?;
    e2ee_repo
        .get_device(id)
        .await?
        .filter(|d| d.user_id == caller.actor.user_id && d.revoked_at.is_none())
        .ok_or_else(not_bound)
}

/// Decode unpadded base64url of at most `max` bytes. Oversized text is refused before it is
/// decoded.
pub(crate) fn decode_b64(field: &str, text: &str, max: usize) -> ServiceResult<Vec<u8>> {
    let refused = || {
        ServiceError::Validation(format!(
            "{field} must be at most {max} bytes of unpadded base64url"
        ))
    };
    if text.len() > max.div_ceil(3) * 4 {
        return Err(refused());
    }
    URL_SAFE_NO_PAD
        .decode(text)
        .ok()
        .filter(|bytes| !bytes.is_empty() && bytes.len() <= max)
        .ok_or_else(refused)
}

/// Exactly `N` bytes of unpadded base64url.
/// Whether a device whose pending Welcomes are `backlog` (`None`: it has none) has room for one
/// more of `size` bytes.
fn welcome_fits(backlog: Option<&MlsWelcomeBacklog>, size: usize) -> bool {
    let size = i64::try_from(size).unwrap_or(i64::MAX);
    backlog.is_none_or(|b| {
        b.count < MAX_PENDING_WELCOMES && b.bytes.saturating_add(size) <= MAX_PENDING_WELCOME_BYTES
    })
}

pub(crate) fn decode_fixed<const N: usize>(field: &str, text: &str) -> ServiceResult<[u8; N]> {
    URL_SAFE_NO_PAD
        .decode(text)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| ServiceError::Validation(format!("{field} must be {N} bytes")))
}

/// Encrypted conversations are DMs and groups.
pub(crate) fn ensure_encryptable(conversation: &Conversation) -> ServiceResult<()> {
    match conversation.conversation_type {
        ConversationType::Direct | ConversationType::Group => Ok(()),
        ConversationType::Channel => Err(ServiceError::Validation(
            "only direct messages and groups are encrypted".to_string(),
        )),
    }
}

/// The start of the MLS-activity window.
pub(crate) fn active_since(now: DateTime<Utc>) -> DateTime<Utc> {
    now - Duration::days(MLS_ACTIVE_DAYS)
}

/// A key package's bytes checked for a device: it parses, its suite is accepted, its credential
/// is the device's `mls-key` statement over the leaf's signature key, and it expires in the
/// future and within 90 days.
fn check_device_key_package(
    wire: &dyn MlsWire,
    bytes: Vec<u8>,
    device: &E2eeDevice,
    principal_id: &str,
    now: DateTime<Utc>,
) -> Result<NewMlsKeyPackage, String> {
    let device_key = rim_identity::PublicKey::parse(&device.public_key, "deviceKey")
        .map_err(|e| e.to_string())?;
    let info = wire.read_key_package(&bytes).map_err(|e| e.0)?;
    if !ACCEPTED_CIPHER_SUITES.contains(&info.cipher_suite) {
        return Err(format!(
            "cipher suite {} is not accepted",
            info.cipher_suite
        ));
    }
    let statement = rim_identity::read_mls_credential(&info.credential_identity).map_err(|e| {
        crate::util::log_statement_detail(&e);
        e.to_string()
    })?;
    let mls_key = rim_identity::verify_mls_key(&device_key, &statement).map_err(|e| {
        crate::util::log_statement_detail(&e);
        format!("mls-key statement: {e}")
    })?;
    if mls_key.principal_id != principal_id || mls_key.device_id != device.device_id {
        return Err("the mls-key statement names another device".to_string());
    }
    if mls_key.mls_key.key_bytes() != info.signature_key {
        return Err("the leaf's signature key is not the mls-key statement's".to_string());
    }
    let not_after = i64::try_from(info.not_after)
        .ok()
        .and_then(|secs| DateTime::from_timestamp(secs, 0))
        .ok_or_else(|| "its lifetime is out of range".to_string())?;
    if not_after <= now || not_after > now + MAX_KEY_PACKAGE_LIFETIME + KEY_PACKAGE_CLOCK_MARGIN {
        return Err("it must expire in the future and within 90 days".to_string());
    }
    Ok(NewMlsKeyPackage {
        user_id: device.user_id,
        device_key_id: device.device_key_id,
        cipher_suite: info.cipher_suite,
        key_package_ref: info.key_package_ref,
        key_package: bytes,
        last_resort: info.last_resort,
        not_after,
    })
}

/// Decode a PrivateMessage of the expected content type.
pub(crate) fn read_private_message(
    wire: &dyn MlsWire,
    bytes: &[u8],
    expected: MlsContentType,
) -> ServiceResult<PrivateMessageInfo> {
    let info = wire
        .read_private_message(bytes)
        .map_err(|e| ServiceError::Validation(e.0))?;
    if info.content_type != expected {
        let what = match expected {
            MlsContentType::Application => "an application message",
            MlsContentType::Commit => "a Commit",
            MlsContentType::Proposal => "a proposal",
        };
        return Err(ServiceError::Validation(format!(
            "the PrivateMessage is not {what}"
        )));
    }
    Ok(info)
}

/// `StaleEpoch` naming the group's current incarnation and epoch.
pub(crate) fn stale(group: &MlsGroup) -> ServiceError {
    ServiceError::StaleEpoch {
        incarnation: group.incarnation,
        epoch: group.epoch,
    }
}

/// Check that a message's group id names this conversation's current incarnation:
/// `StaleEpoch` for an older incarnation, `Validation` for anything else.
pub(crate) fn check_group_id(
    conversation_id: ConversationId,
    group: &MlsGroup,
    group_id: &[u8],
) -> ServiceResult<()> {
    if group_id == group.group_id.as_slice() {
        return Ok(());
    }
    match parse_mls_group_id(group_id) {
        Some((conversation, incarnation))
            if conversation == conversation_id
                && i64::from(incarnation) < i64::from(group.incarnation) =>
        {
            Err(stale(group))
        }
        _ => Err(ServiceError::Validation(
            "the group id does not name this conversation's group".to_string(),
        )),
    }
}

/// A group and the devices it holds, should hold, and should re-add.
pub(crate) struct GroupState {
    pub group: MlsGroup,
    pub devices: Vec<MlsGroupDevice>,
    pub desired: Vec<MlsDesiredDevice>,
    /// Open re-add requests of the group's incarnation.
    pub readd_requests: Vec<MlsReaddRequest>,
}

impl GroupState {
    pub(crate) async fn load(
        e2ee_repo: &dyn E2eeRepository,
        group: MlsGroup,
        now: DateTime<Utc>,
    ) -> ServiceResult<Self> {
        let devices = e2ee_repo
            .group_devices(group.conversation_id, group.incarnation)
            .await?;
        let desired = e2ee_repo
            .desired_devices(group.conversation_id, active_since(now), group.cipher_suite)
            .await?;
        let readd_requests = e2ee_repo
            .open_readd_requests(
                group.conversation_id,
                group.incarnation,
                now - READD_REQUEST_TTL,
            )
            .await?;
        Ok(Self {
            group,
            devices,
            desired,
            readd_requests,
        })
    }

    pub(crate) fn is_device(&self, id: DeviceKeyId) -> bool {
        self.devices.iter().any(|d| d.device.device_key_id == id)
    }

    pub(crate) fn is_desired(&self, id: DeviceKeyId) -> bool {
        self.desired.iter().any(|d| d.device.device_key_id == id)
    }

    /// Desired devices that are not leaves.
    pub(crate) fn pending_adds(&self) -> impl Iterator<Item = &MlsDesiredDevice> {
        self.desired
            .iter()
            .filter(|d| !self.is_device(d.device.device_key_id))
    }

    /// Leaves that are no longer desired.
    pub(crate) fn pending_removes(&self) -> impl Iterator<Item = &MlsGroupDevice> {
        self.devices
            .iter()
            .filter(|d| !self.is_desired(d.device.device_key_id))
    }

    /// Desired leaves with an open re-add request, with when they last asked.
    pub(crate) fn readd_targets(&self) -> HashMap<DeviceKeyId, DateTime<Utc>> {
        let mut targets = HashMap::new();
        for request in &self.readd_requests {
            let id = request.device.device_key_id;
            if self.is_device(id) && self.is_desired(id) {
                let latest = targets.entry(id).or_insert(request.created_at);
                *latest = (*latest).max(request.created_at);
            }
        }
        targets
    }

    fn has_desired_device(&self) -> bool {
        self.devices
            .iter()
            .any(|d| self.is_desired(d.device.device_key_id))
    }
}

#[derive(Clone)]
pub struct E2eeRepos {
    pub e2ee_repo: Arc<dyn E2eeRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub key_identity_repo: Arc<dyn KeyIdentityRepository>,
    pub message_repo: Arc<dyn MessageRepository>,
    pub moderation_repo: Arc<dyn ModerationRepository>,
}

pub struct DefaultE2eeService {
    repos: E2eeRepos,
    /// When set, uploads, claims and Commits run in one transaction each.
    db: Option<DatabaseConnection>,
    wire: Arc<dyn MlsWire>,
    observers: Observers,
    audit_log_service: Option<Arc<dyn AuditLogService>>,
    /// Devices claimed per user.
    claim_rate_limiter: RegistrationRateLimiter,
    /// One-time key packages handed out per target device; past it only the last-resort one.
    one_time_claim_rate_limiter: RegistrationRateLimiter,
    /// Re-add requests per device and conversation.
    readd_rate_limiter: RegistrationRateLimiter,
    /// Claims key packages of remote users' devices from their relays.
    remote_key_packages: Option<Arc<dyn RemoteKeyPackages>>,
    /// Verifies franking receipts. `None` in unit tests that are not about reports; the
    /// composition root always wires it.
    franking_keys: Option<SharedFrankingKeys>,
    /// Most unclaimed one-time key packages per device and suite.
    key_packages_per_device: i64,
}

/// Why an incarnation's first Commit is allowed over an existing group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IncarnationGround {
    SuiteMigration,
    Recovery,
    NoDevicesLeft,
}

/// What an accepted Commit changed, for the events after it commits.
struct Accepted {
    commit: MlsCommit,
    welcomed: Vec<(UserId, uuid::Uuid)>,
    membership_changed: bool,
    newly_encrypted: bool,
}

fn audit_entry(
    actor: UserId,
    conversation: &Conversation,
    command_id: &'static str,
    args: serde_json::Value,
) -> ModerationAuditEntry {
    ModerationAuditEntry {
        actor,
        server_id: conversation.server_id,
        command_id,
        args,
        dangerous: false,
        typed_verb: TypedVerb::Write,
    }
}

impl DefaultE2eeService {
    pub fn new(
        repos: E2eeRepos,
        db: Option<DatabaseConnection>,
        wire: Arc<dyn MlsWire>,
        observers: Observers,
    ) -> Self {
        Self {
            repos,
            db,
            wire,
            observers,
            audit_log_service: None,
            claim_rate_limiter: RegistrationRateLimiter::new(3600, 600),
            one_time_claim_rate_limiter: RegistrationRateLimiter::new(86_400, 100),
            readd_rate_limiter: RegistrationRateLimiter::new(86_400, 3),
            franking_keys: None,
            remote_key_packages: None,
            key_packages_per_device: MAX_AVAILABLE_KEY_PACKAGES,
        }
    }

    /// Cap unclaimed one-time key packages per device and suite at `cap`.
    pub fn with_key_package_cap(mut self, cap: i64) -> Self {
        self.key_packages_per_device = cap.max(1);
        self
    }

    pub fn with_franking_keys(mut self, franking_keys: SharedFrankingKeys) -> Self {
        self.franking_keys = Some(franking_keys);
        self
    }

    pub fn with_remote_key_packages(mut self, remote: Arc<dyn RemoteKeyPackages>) -> Self {
        self.remote_key_packages = Some(remote);
        self
    }

    pub fn with_audit_log_service(mut self, audit_log_service: Arc<dyn AuditLogService>) -> Self {
        self.audit_log_service = Some(audit_log_service);
        self
    }

    /// Replace the limiters (claims per user, one-time claims per target device, re-add
    /// requests per device and conversation).
    #[cfg(test)]
    pub(crate) fn with_rate_limiters(
        mut self,
        claims: RegistrationRateLimiter,
        one_time_claims: RegistrationRateLimiter,
        readds: RegistrationRateLimiter,
    ) -> Self {
        self.claim_rate_limiter = claims;
        self.one_time_claim_rate_limiter = one_time_claims;
        self.readd_rate_limiter = readds;
        self
    }

    fn access_repos(&self) -> ConversationAccessRepos<'_> {
        ConversationAccessRepos::from_arcs(
            &self.repos.server_member_repo,
            &self.repos.conversation_repo,
            &self.repos.conversation_member_repo,
        )
    }

    /// Open a transaction when a database is configured, and the repos that run inside it.
    async fn begin(&self) -> ServiceResult<(Option<TransactionScope>, E2eeRepos)> {
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_e2ee(),
            None => self.repos.clone(),
        };
        Ok((scope, repos))
    }

    async fn commit_scope(scope: Option<TransactionScope>) -> ServiceResult<()> {
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        Ok(())
    }

    fn limit(
        limiter: &RegistrationRateLimiter,
        bucket: &str,
        key: &'static str,
    ) -> ServiceResult<()> {
        match limiter.check_and_record(bucket, key, Utc::now()) {
            RateLimitDecision::Allowed => Ok(()),
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => {
                crate::metrics::rate_limited(key);
                Err(ServiceError::RateLimited {
                    retry_after_seconds,
                })
            }
        }
    }

    async fn audit(&self, entry: ModerationAuditEntry) {
        record_audit_action(self.audit_log_service.as_ref(), entry, AUDIT_INITIATOR).await;
    }

    async fn member_ids(&self, conversation_id: ConversationId) -> ServiceResult<Vec<UserId>> {
        Ok(self
            .repos
            .conversation_member_repo
            .list_member_ids(conversation_id)
            .await?)
    }

    async fn emit_membership_changed(&self, conversation: &Conversation) {
        match self.member_ids(conversation.id).await {
            Ok(member_user_ids) => self.observers.emit(WorkspaceEvent::MlsMembershipChanged {
                workspace_id: conversation.server_id,
                conversation_id: conversation.id,
                member_user_ids,
            }),
            Err(e) => {
                tracing::warn!(error = %e, conversation_id = %conversation.id, "could not list members for an MLS event");
            }
        }
    }

    fn commit_dto(commit: MlsCommit) -> MlsCommitDto {
        MlsCommitDto {
            id: commit.id,
            conversation_id: commit.conversation_id,
            incarnation: commit.incarnation,
            epoch: commit.epoch,
            sender_principal_id: commit.sender_principal_id,
            sender_device_id: commit.sender_device_id,
            message: commit.message,
            created_at: commit.created_at,
        }
    }

    fn status_dto(status: MlsKeyPackageStatus) -> MlsKeyPackageStatusDto {
        MlsKeyPackageStatusDto {
            cipher_suite: status.cipher_suite,
            available: status.available,
            last_resort_not_after: status.last_resort_not_after,
            last_resort_claims: status.last_resort_claims,
            oldest_not_after: status.oldest_not_after,
        }
    }

    /// Whether `device` may start a new incarnation over `state`, and on what ground.
    async fn incarnation_ground(
        repos: &E2eeRepos,
        state: &GroupState,
        device: DeviceKeyId,
        cipher_suite: Option<i32>,
        now: DateTime<Utc>,
    ) -> ServiceResult<Option<IncarnationGround>> {
        if state.is_device(device) {
            if cipher_suite.is_none_or(|suite| suite == PREFERRED_CIPHER_SUITE)
                && state.group.cipher_suite != PREFERRED_CIPHER_SUITE
            {
                return Ok(Some(IncarnationGround::SuiteMigration));
            }
            let waited = state
                .readd_requests
                .iter()
                .filter(|r| r.device.device_key_id == device && r.created_at <= now - RECOVERY_WAIT)
                .map(|r| r.created_at)
                .min();
            if let Some(requested_at) = waited
                && !repos
                    .e2ee_repo
                    .commit_accepted_since(
                        state.group.conversation_id,
                        state.group.incarnation,
                        requested_at,
                    )
                    .await?
            {
                return Ok(Some(IncarnationGround::Recovery));
            }
        }
        if !state.has_desired_device() && state.is_desired(device) {
            return Ok(Some(IncarnationGround::NoDevicesLeft));
        }
        Ok(None)
    }

    /// Claim the targets that belong to remote users from their relays. Their packages are
    /// checked like uploads and returned to store as claimed; the local targets are returned
    /// for the usual claim.
    #[allow(clippy::too_many_arguments)]
    async fn claim_remote(
        &self,
        claimer: &E2eeDevice,
        conversation: &Conversation,
        incarnation: i32,
        suite: i32,
        targets: Vec<MlsDevice>,
        packages: &mut Vec<NewMlsKeyPackage>,
        claimed: &mut Vec<ClaimedKeyPackageDto>,
    ) -> ServiceResult<Vec<MlsDevice>> {
        let Some(remote) = &self.remote_key_packages else {
            return Ok(targets);
        };
        let claimer_principal = claimer.principal_id.clone().ok_or_else(not_bound)?;
        let mut by_user: Vec<(UserId, Vec<MlsDevice>)> = Vec::new();
        for target in targets {
            match by_user.iter_mut().find(|(u, _)| *u == target.user_id) {
                Some((_, devices)) => devices.push(target),
                None => by_user.push((target.user_id, vec![target])),
            }
        }
        let now = Utc::now();
        let mut local = Vec::new();
        for (user_id, devices) in by_user {
            let answer = remote
                .claim(RemoteClaim {
                    target_user_id: user_id,
                    device_ids: devices.iter().map(|d| d.device_id).collect(),
                    conversation_id: conversation.id,
                    incarnation,
                    claimer_principal_id: claimer_principal.clone(),
                    claimer_device_id: claimer.device_id,
                    cipher_suite: suite,
                })
                .await?;
            let Some(answer) = answer else {
                local.extend(devices);
                continue;
            };
            for package in answer {
                let Some(target) = devices.iter().find(|d| d.device_id == package.device_id) else {
                    continue;
                };
                let Some(stored) = self
                    .repos
                    .e2ee_repo
                    .get_device(target.device_key_id)
                    .await?
                else {
                    continue;
                };
                match check_device_key_package(
                    self.wire.as_ref(),
                    package.key_package,
                    &stored,
                    &target.principal_id,
                    now,
                ) {
                    Ok(checked) if checked.cipher_suite == suite => {
                        claimed.push(ClaimedKeyPackageDto {
                            user_id: target.user_id,
                            principal_id: target.principal_id.clone(),
                            device_id: target.device_id,
                            key_package: checked.key_package.clone(),
                            key_package_ref: checked.key_package_ref.clone(),
                            last_resort: checked.last_resort,
                        });
                        packages.push(checked);
                    }
                    Ok(_) => {
                        tracing::warn!(device_id = %target.device_id, "a relay handed out a key package of another suite")
                    }
                    Err(why) => {
                        tracing::warn!(device_id = %target.device_id, why, "a relay handed out a key package that does not check")
                    }
                }
            }
        }
        Ok(local)
    }

    /// Who may create a conversation's first group: any member of a DM; in a group the
    /// creator, unless the creator is gone. A conversation created encrypted gives its creator
    /// a head start before any member may.
    fn may_create_group(
        conversation: &Conversation,
        actor: UserId,
        members: &[UserId],
        now: DateTime<Utc>,
    ) -> bool {
        let creator = conversation
            .created_by
            .filter(|creator| members.contains(creator));
        let Some(creator) = creator else {
            return true;
        };
        if creator == actor {
            return true;
        }
        match conversation.encrypted_at {
            // Created encrypted: anyone once the creator had their head start.
            Some(_) => now - conversation.created_at >= CREATOR_HEAD_START,
            // A plaintext group is upgraded by its creator.
            None => conversation.conversation_type == ConversationType::Direct,
        }
    }

    /// The key package a Commit adds a device with: claimed for this conversation and
    /// incarnation by the committer within the hour, of the device and the group's suite. A
    /// re-add, asked for at `readd_since`, needs a one-time package uploaded after the request.
    #[allow(clippy::too_many_arguments)]
    fn check_key_package(
        package: Option<&MlsKeyPackage>,
        device: DeviceKeyId,
        committer: DeviceKeyId,
        conversation_id: ConversationId,
        incarnation: i32,
        cipher_suite: i32,
        readd_since: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> ServiceResult<()> {
        let refused =
            |why: &str| ServiceError::Validation(format!("an added device's key package {why}"));
        let package = package.ok_or_else(|| refused("is unknown"))?;
        if package.device_key_id != device {
            return Err(refused("belongs to another device"));
        }
        if package.cipher_suite != cipher_suite {
            return Err(refused("is of another cipher suite"));
        }
        if let Some(since) = readd_since {
            if package.last_resort {
                return Err(refused("must be a one-time package for a re-add"));
            }
            if package.created_at <= since {
                return Err(refused("was uploaded before the device's re-add request"));
            }
        }
        let claimed = package.claimed_by_device_key_id == Some(committer)
            && package.claimed_for_conversation_id == Some(conversation_id)
            && package
                .claimed_at
                .is_some_and(|at| at >= now - CLAIM_VALIDITY);
        if !claimed {
            return Err(refused(
                "was not claimed for this conversation by this device in the last hour",
            ));
        }
        if package.claimed_for_incarnation != Some(incarnation) {
            return Err(refused("was claimed for another incarnation"));
        }
        Ok(())
    }

    /// The Commit transaction: lock the group, check every rule against its state, store the
    /// Commit, advance the epoch, and apply the summary.
    #[allow(clippy::too_many_arguments)]
    async fn accept_commit(
        &self,
        repos: &E2eeRepos,
        cmd: &SubmitMlsCommitCmd,
        conversation: &Conversation,
        committer: &E2eeDevice,
        header: &PrivateMessageInfo,
        summary: &CommitAad,
        commit_bytes: Vec<u8>,
        welcome: Option<(Vec<u8>, Vec<Vec<u8>>, i32)>,
        now: DateTime<Utc>,
    ) -> ServiceResult<Accepted> {
        let conversation_id = conversation.id;
        let members = repos
            .conversation_member_repo
            .list_member_ids(conversation_id)
            .await?;
        if members.len() > MAX_ENCRYPTED_MEMBERS {
            return Err(ServiceError::Validation(format!(
                "an encrypted conversation has at most {MAX_ENCRYPTED_MEMBERS} members"
            )));
        }
        let current = repos
            .e2ee_repo
            .lock_current_group(conversation_id, GroupLock::Update)
            .await?;
        let current = match current {
            Some(group) => Some(GroupState::load(repos.e2ee_repo.as_ref(), group, now).await?),
            None => None,
        };
        let first = cmd.epoch == 0;
        let (incarnation, cipher_suite) = if first {
            let next = current.as_ref().map_or(1, |s| s.group.incarnation + 1);
            if cmd.incarnation != next {
                return Err(match &current {
                    Some(state) if cmd.incarnation <= state.group.incarnation => {
                        stale(&state.group)
                    }
                    _ => ServiceError::Validation(format!(
                        "the next incarnation is {next}, not {}",
                        cmd.incarnation
                    )),
                });
            }
            let suite = cmd.cipher_suite.ok_or_else(|| {
                ServiceError::Validation(
                    "cipherSuite is required for an incarnation's first Commit".to_string(),
                )
            })?;
            if !ACCEPTED_CIPHER_SUITES.contains(&suite) {
                return Err(ServiceError::Validation(format!(
                    "cipher suite {suite} is not accepted"
                )));
            }
            (next, suite)
        } else {
            let state = current.as_ref().ok_or_else(|| {
                ServiceError::Validation("the conversation has no MLS group yet".to_string())
            })?;
            if cmd.incarnation != state.group.incarnation {
                return Err(if cmd.incarnation < state.group.incarnation {
                    stale(&state.group)
                } else {
                    ServiceError::Validation(format!(
                        "incarnation {} does not exist",
                        cmd.incarnation
                    ))
                });
            }
            if cmd
                .cipher_suite
                .is_some_and(|suite| suite != state.group.cipher_suite)
            {
                return Err(ServiceError::Validation(
                    "cipherSuite differs from the group's".to_string(),
                ));
            }
            (state.group.incarnation, state.group.cipher_suite)
        };

        // The header must agree with the request.
        let expected_group_id = mls_group_id(conversation_id, incarnation.unsigned_abs());
        if header.group_id != expected_group_id {
            return Err(match &current {
                Some(state) => {
                    check_group_id(conversation_id, &state.group, &header.group_id)?;
                    ServiceError::Validation(
                        "the group id does not name the requested incarnation".to_string(),
                    )
                }
                None => ServiceError::Validation(
                    "the group id does not name this conversation's group".to_string(),
                ),
            });
        }
        if i64::try_from(header.epoch).ok() != Some(cmd.epoch) {
            return Err(ServiceError::Validation(
                "the Commit's epoch differs from the requested one".to_string(),
            ));
        }
        if !first
            && let Some(state) = &current
            && cmd.epoch != state.group.epoch
        {
            return Err(stale(&state.group));
        }

        // Who the summary names.
        let desired: Vec<MlsDesiredDevice> = match &current {
            Some(state) => state.desired.clone(),
            None => {
                repos
                    .e2ee_repo
                    .desired_devices(conversation_id, active_since(now), cipher_suite)
                    .await?
            }
        };
        let find_desired = |principal_id: &str, device_id: uuid::Uuid| {
            desired
                .iter()
                .find(|d| d.device.principal_id == principal_id && d.device.device_id == device_id)
        };
        let committer_id = committer.device_key_id;

        let mut newly_encrypted = false;
        let mut readded = Vec::new();
        let mut readd_since = HashMap::new();
        let mut removed_ids = Vec::new();
        let mut added = Vec::with_capacity(summary.adds.len());
        for add in &summary.adds {
            let device = find_desired(&add.principal_id, add.device_id).ok_or_else(|| {
                ServiceError::Validation(format!(
                    "device {} of {} is not a device of a member that may be added",
                    add.device_id, add.principal_id
                ))
            })?;
            if device.device.device_key_id == committer_id {
                return Err(ServiceError::Validation(
                    "the committer cannot add itself".to_string(),
                ));
            }
            added.push((device.device.clone(), add.key_package_ref.clone()));
        }

        if first {
            if !summary.removes.is_empty() {
                return Err(ServiceError::Validation(
                    "an incarnation's first Commit removes nothing".to_string(),
                ));
            }
            match &current {
                None => {
                    if !desired
                        .iter()
                        .any(|d| d.device.device_key_id == committer_id)
                    {
                        return Err(ServiceError::Validation(
                            "this device must upload key packages before it creates a group"
                                .to_string(),
                        ));
                    }
                    if !Self::may_create_group(
                        conversation,
                        cmd.caller.actor.user_id,
                        &members,
                        now,
                    ) {
                        return Err(ServiceError::Forbidden(
                            "the conversation's creator creates its group first".to_string(),
                        ));
                    }
                }
                Some(state) => {
                    let ground = Self::incarnation_ground(
                        repos,
                        state,
                        committer_id,
                        Some(cipher_suite),
                        now,
                    )
                    .await?;
                    match ground {
                        Some(_) => {}
                        None if state.is_device(committer_id) => {
                            return Err(ServiceError::Validation(
                                "a new incarnation needs a suite migration, a recovery, or a \
                                 group with no devices left"
                                    .to_string(),
                            ));
                        }
                        None => {
                            return Err(ServiceError::Forbidden(
                                "this device is not in the conversation's group".to_string(),
                            ));
                        }
                    }
                }
            }
            let mut in_group: HashSet<UserId> = HashSet::from([committer.user_id]);
            in_group.extend(added.iter().map(|(d, _)| d.user_id));
            if members.iter().any(|m| !in_group.contains(m)) {
                return Err(ServiceError::Validation(
                    "every member needs at least one device in the group".to_string(),
                ));
            }
            if added.len() + 1 > MAX_GROUP_DEVICES {
                return Err(ServiceError::Validation(format!(
                    "a group holds at most {MAX_GROUP_DEVICES} devices"
                )));
            }
        } else {
            let Some(state) = &current else {
                return Err(ServiceError::Validation(
                    "the conversation has no MLS group yet".to_string(),
                ));
            };
            if !state.is_device(committer_id) {
                return Err(ServiceError::Forbidden(
                    "this device is not in the conversation's group".to_string(),
                ));
            }
            if state
                .pending_removes()
                .any(|d| d.device.device_key_id == committer_id)
            {
                return Err(ServiceError::Forbidden(
                    "this device is waiting to be removed from the group".to_string(),
                ));
            }
            let readd_targets = state.readd_targets();
            let added_ids: HashSet<DeviceKeyId> =
                added.iter().map(|(d, _)| d.device_key_id).collect();
            for remove in &summary.removes {
                let device = state
                    .devices
                    .iter()
                    .find(|d| {
                        d.device.principal_id == remove.principal_id
                            && d.device.device_id == remove.device_id
                    })
                    .ok_or_else(|| {
                        ServiceError::Validation(format!(
                            "device {} of {} is not in the group",
                            remove.device_id, remove.principal_id
                        ))
                    })?;
                let id = device.device.device_key_id;
                if id == committer_id {
                    return Err(ServiceError::Validation(
                        "the committer cannot remove itself".to_string(),
                    ));
                }
                let readd = readd_targets.contains_key(&id) && added_ids.contains(&id);
                if state.is_desired(id) && !readd {
                    return Err(ServiceError::Validation(format!(
                        "device {} of {} is not waiting to be removed",
                        remove.device_id, remove.principal_id
                    )));
                }
                if readd {
                    readded.push(id);
                    readd_since.insert(id, readd_targets[&id]);
                }
                removed_ids.push(id);
            }
            for (device, _) in &added {
                let id = device.device_key_id;
                if state.is_device(id) && !readded.contains(&id) {
                    return Err(ServiceError::Validation(format!(
                        "device {} of {} is already in the group",
                        device.device_id, device.principal_id
                    )));
                }
            }
            let resulting = state.devices.len() - removed_ids.len() + added.len();
            if resulting > MAX_GROUP_DEVICES {
                return Err(ServiceError::Validation(format!(
                    "a group holds at most {MAX_GROUP_DEVICES} devices"
                )));
            }
        }

        // Key packages and the Welcome.
        let refs: Vec<Vec<u8>> = added.iter().map(|(_, r)| r.clone()).collect();
        let packages = repos.e2ee_repo.find_key_packages_by_refs(&refs).await?;
        for (device, key_package_ref) in &added {
            let package = packages
                .iter()
                .find(|p| &p.key_package_ref == key_package_ref);
            Self::check_key_package(
                package,
                device.device_key_id,
                committer_id,
                conversation_id,
                incarnation,
                cipher_suite,
                readd_since.get(&device.device_key_id).copied(),
                now,
            )?;
        }
        match (&welcome, added.is_empty()) {
            (None, false) => {
                return Err(ServiceError::Validation(
                    "a Commit that adds devices needs a Welcome".to_string(),
                ));
            }
            (Some(_), true) => {
                return Err(ServiceError::Validation(
                    "a Commit that adds no device has no Welcome".to_string(),
                ));
            }
            (Some((_, welcome_refs, welcome_suite)), false) => {
                let expected: HashSet<&Vec<u8>> = refs.iter().collect();
                let named: HashSet<&Vec<u8>> = welcome_refs.iter().collect();
                if *welcome_suite != cipher_suite
                    || named != expected
                    || welcome_refs.len() != refs.len()
                {
                    return Err(ServiceError::Validation(
                        "the Welcome is not for exactly the added devices".to_string(),
                    ));
                }
            }
            (None, true) => {}
        }
        if let Some((message, _, _)) = &welcome {
            let ids: Vec<DeviceKeyId> = added.iter().map(|(d, _)| d.device_key_id).collect();
            let backlog = repos.e2ee_repo.welcome_backlog(&ids).await?;
            let full = added.iter().find(|(d, _)| {
                let pending = backlog.iter().find(|b| b.device_key_id == d.device_key_id);
                !welcome_fits(pending, message.len())
            });
            if let Some((device, _)) = full {
                return Err(ServiceError::Validation(format!(
                    "device {} of {} has too many pending Welcomes",
                    device.device_id, device.principal_id
                )));
            }
        }

        // Writes.
        let stale_after_conflict = |e: rim_core::repository::error::RepoError| {
            if e.kind() == RepoErrorKind::Conflict {
                // The losing side of a race; the caller re-reads the group.
                ServiceError::StaleEpoch {
                    incarnation,
                    epoch: cmd.epoch + 1,
                }
            } else {
                e.into()
            }
        };
        if first {
            if let Some(state) = &current {
                repos
                    .e2ee_repo
                    .retire_group(conversation_id, state.group.incarnation)
                    .await?;
            }
            repos
                .e2ee_repo
                .insert_group(NewMlsGroup {
                    conversation_id,
                    incarnation,
                    group_id: expected_group_id,
                    cipher_suite,
                    epoch: 0,
                    created_by_device_key_id: Some(committer_id),
                })
                .await
                .map_err(stale_after_conflict)?;
        }
        let commit = repos
            .e2ee_repo
            .insert_commit(NewMlsCommit {
                conversation_id,
                incarnation,
                epoch: cmd.epoch,
                sender_user_id: committer.user_id,
                sender_device_key_id: committer_id,
                message: commit_bytes,
            })
            .await
            .map_err(stale_after_conflict)?;
        let new_epoch = repos
            .e2ee_repo
            .advance_epoch(conversation_id, incarnation)
            .await?;
        if first {
            repos
                .e2ee_repo
                .add_group_devices(
                    conversation_id,
                    incarnation,
                    0,
                    &[(committer_id, committer.user_id)],
                )
                .await?;
            if incarnation == 1 {
                newly_encrypted = repos
                    .e2ee_repo
                    .mark_conversation_encrypted(conversation_id)
                    .await?;
            }
        }
        repos
            .e2ee_repo
            .remove_group_devices(conversation_id, incarnation, &removed_ids)
            .await?;
        let added_pairs: Vec<(DeviceKeyId, UserId)> = added
            .iter()
            .map(|(d, _)| (d.device_key_id, d.user_id))
            .collect();
        repos
            .e2ee_repo
            .add_group_devices(conversation_id, incarnation, new_epoch, &added_pairs)
            .await?;
        repos
            .e2ee_repo
            .fulfill_readd_requests(conversation_id, incarnation, &readded, new_epoch)
            .await?;
        let used: Vec<_> = packages.iter().map(|p| p.id).collect();
        repos.e2ee_repo.consume_key_packages(&used).await?;
        if let Some((message, _, _)) = welcome {
            repos
                .e2ee_repo
                .insert_welcome(NewMlsWelcome {
                    commit_id: commit.id,
                    conversation_id,
                    incarnation,
                    epoch: new_epoch,
                    message,
                    expires_at: now + WELCOME_TTL,
                    recipients: added_pairs,
                })
                .await?;
        }
        Ok(Accepted {
            commit,
            welcomed: added
                .iter()
                .map(|(d, _)| (d.user_id, d.device_id))
                .collect(),
            membership_changed: first || !summary.adds.is_empty() || !summary.removes.is_empty(),
            newly_encrypted,
        })
    }
}

#[async_trait]
impl E2eeService for DefaultE2eeService {
    async fn key_package_status(
        &self,
        caller: MlsCaller,
    ) -> ServiceResult<Vec<MlsKeyPackageStatusDto>> {
        let device = session_device(self.repos.e2ee_repo.as_ref(), &caller).await?;
        Ok(self
            .repos
            .e2ee_repo
            .key_package_status(device.device_key_id)
            .await?
            .into_iter()
            .map(Self::status_dto)
            .collect())
    }

    #[instrument(name = "e2ee.upload_key_packages", skip_all, fields(actor_id = %cmd.caller.actor.user_id))]
    async fn upload_key_packages(
        &self,
        cmd: UploadMlsKeyPackagesCmd,
    ) -> ServiceResult<MlsKeyPackageStatusDto> {
        let device = session_device(self.repos.e2ee_repo.as_ref(), &cmd.caller).await?;
        if cmd.key_packages.is_empty() || cmd.key_packages.len() > MAX_KEY_PACKAGES_PER_UPLOAD {
            return Err(ServiceError::Validation(format!(
                "upload 1 to {MAX_KEY_PACKAGES_PER_UPLOAD} key packages"
            )));
        }
        let principal_id = device.principal_id.clone().ok_or_else(|| {
            ServiceError::Validation("this account has no key identity".to_string())
        })?;
        let now = Utc::now();
        let mut packages = Vec::with_capacity(cmd.key_packages.len());
        let mut refs = HashSet::new();
        let mut last_resort_suites = HashSet::new();
        for (index, text) in cmd.key_packages.iter().enumerate() {
            let refused =
                |why: String| ServiceError::Validation(format!("key package {index}: {why}"));
            let bytes = decode_b64("keyPackage", text, MAX_KEY_PACKAGE_BYTES)
                .map_err(|e| refused(e.to_string()))?;
            let package =
                check_device_key_package(self.wire.as_ref(), bytes, &device, &principal_id, now)
                    .map_err(refused)?;
            if !refs.insert(package.key_package_ref.clone()) {
                return Err(refused("it is uploaded twice".to_string()));
            }
            if package.last_resort && !last_resort_suites.insert(package.cipher_suite) {
                return Err(refused(
                    "an upload carries one last-resort package per cipher suite".to_string(),
                ));
            }
            packages.push(package);
        }

        let (scope, repos) = self.begin().await?;
        if !repos.e2ee_repo.lock_device(device.device_key_id).await? {
            return Err(not_bound());
        }
        for suite in &last_resort_suites {
            repos
                .e2ee_repo
                .delete_last_resort_key_package(device.device_key_id, *suite)
                .await?;
        }
        let suites: HashSet<i32> = packages.iter().map(|p| p.cipher_suite).collect();
        for (index, package) in packages.into_iter().enumerate() {
            repos
                .e2ee_repo
                .insert_key_package(package)
                .await
                .map_err(|e| match e.kind() {
                    RepoErrorKind::Conflict => ServiceError::Validation(format!(
                        "key package {index}: it was already uploaded"
                    )),
                    _ => e.into(),
                })?;
        }
        for suite in suites {
            let available = repos
                .e2ee_repo
                .count_available_key_packages(device.device_key_id, suite)
                .await?;
            if available > self.key_packages_per_device {
                return Err(ServiceError::Validation(format!(
                    "a device keeps at most {} unclaimed key packages",
                    self.key_packages_per_device
                )));
            }
        }
        repos
            .e2ee_repo
            .touch_key_package_upload(device.device_key_id)
            .await?;
        let status = repos
            .e2ee_repo
            .key_package_status(device.device_key_id)
            .await?
            .into_iter()
            .find(|s| s.cipher_suite == PREFERRED_CIPHER_SUITE)
            .map(Self::status_dto)
            .unwrap_or(MlsKeyPackageStatusDto {
                cipher_suite: PREFERRED_CIPHER_SUITE,
                available: 0,
                last_resort_not_after: None,
                last_resort_claims: 0,
                oldest_not_after: None,
            });
        Self::commit_scope(scope).await?;
        info!(device_key_id = %device.device_key_id, count = cmd.key_packages.len(), "stored MLS key packages");

        // The device may now be waiting to be added to the user's encrypted conversations.
        let in_groups: HashSet<ConversationId> = self
            .repos
            .e2ee_repo
            .conversations_with_group_device(device.device_key_id)
            .await?
            .into_iter()
            .collect();
        let waiting: Vec<ConversationId> = self
            .repos
            .e2ee_repo
            .grouped_conversations_of_user(device.user_id)
            .await?
            .into_iter()
            .filter(|id| !in_groups.contains(id))
            .collect();
        for conversation in self.repos.conversation_repo.get_by_ids(&waiting).await? {
            self.emit_membership_changed(&conversation).await;
        }
        Ok(status)
    }

    #[instrument(name = "e2ee.claim_key_packages", skip_all, fields(actor_id = %cmd.caller.actor.user_id, conversation_id = %cmd.conversation_id))]
    async fn claim_key_packages(
        &self,
        cmd: ClaimMlsKeyPackagesCmd,
    ) -> ServiceResult<Vec<ClaimedKeyPackageDto>> {
        let device = session_device(self.repos.e2ee_repo.as_ref(), &cmd.caller).await?;
        let (conversation, _) = ensure_conversation_writable_with_standing(
            self.access_repos(),
            cmd.caller.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;
        ensure_encryptable(&conversation)?;
        if cmd.devices.is_empty() || cmd.devices.len() > MAX_GROUP_DEVICES {
            return Err(ServiceError::Validation(format!(
                "claim 1 to {MAX_GROUP_DEVICES} devices"
            )));
        }
        if cmd.devices.iter().collect::<HashSet<_>>().len() != cmd.devices.len() {
            return Err(ServiceError::Validation(
                "a device is named twice".to_string(),
            ));
        }
        let now = Utc::now();
        // Targets in the group's incarnation and suite, or, for a new group or incarnation, any
        // desired device in the preferred suite.
        let (incarnation, suite, desired, readd_targets) = match self
            .repos
            .e2ee_repo
            .current_group(conversation.id)
            .await?
        {
            None => {
                let desired = self
                    .repos
                    .e2ee_repo
                    .desired_devices(conversation.id, active_since(now), PREFERRED_CIPHER_SUITE)
                    .await?;
                (1, PREFERRED_CIPHER_SUITE, desired, HashMap::new())
            }
            Some(group) => {
                let state = GroupState::load(self.repos.e2ee_repo.as_ref(), group, now).await?;
                let ground =
                    Self::incarnation_ground(&self.repos, &state, device.device_key_id, None, now)
                        .await?;
                if ground.is_none() && !state.is_device(device.device_key_id) {
                    return Err(ServiceError::Forbidden(
                        "this device is not in the conversation's group".to_string(),
                    ));
                }
                if ground.is_some() {
                    (
                        state.group.incarnation + 1,
                        PREFERRED_CIPHER_SUITE,
                        state.desired.clone(),
                        HashMap::new(),
                    )
                } else {
                    let readd_targets = state.readd_targets();
                    let pending: Vec<MlsDesiredDevice> = state
                        .desired
                        .iter()
                        .filter(|d| {
                            !state.is_device(d.device.device_key_id)
                                || readd_targets.contains_key(&d.device.device_key_id)
                        })
                        .cloned()
                        .collect();
                    (
                        state.group.incarnation,
                        state.group.cipher_suite,
                        pending,
                        readd_targets,
                    )
                }
            }
        };
        let user_key = cmd.caller.actor.user_id.to_string();
        for _ in &cmd.devices {
            Self::limit(&self.claim_rate_limiter, &user_key, CLAIM_KEY)?;
        }

        let mut targets = Vec::with_capacity(cmd.devices.len());
        for target in &cmd.devices {
            let device_ref = desired
                .iter()
                .find(|d| {
                    d.device.principal_id == target.principal_id
                        && d.device.device_id == target.device_id
                        && d.device.device_key_id != device.device_key_id
                })
                .ok_or_else(|| {
                    ServiceError::Validation(format!(
                        "device {} of {} is not waiting to be added",
                        target.device_id, target.principal_id
                    ))
                })?;
            targets.push(device_ref.device.clone());
        }
        // A device without room for one more Welcome of the largest size gets no package, so
        // the claimer leaves it out of its Commit.
        let target_ids: Vec<DeviceKeyId> = targets.iter().map(|t| t.device_key_id).collect();
        let backlog = self.repos.e2ee_repo.welcome_backlog(&target_ids).await?;
        targets.retain(|t| {
            let pending = backlog.iter().find(|b| b.device_key_id == t.device_key_id);
            welcome_fits(pending, MAX_WELCOME_BYTES)
        });

        let mut claimed = Vec::with_capacity(targets.len());
        let mut remote_packages = Vec::new();
        let targets = self
            .claim_remote(
                &device,
                &conversation,
                incarnation,
                suite,
                targets,
                &mut remote_packages,
                &mut claimed,
            )
            .await?;
        let (scope, repos) = self.begin().await?;
        for package in remote_packages {
            repos
                .e2ee_repo
                .store_claimed_key_package(
                    package,
                    device.device_key_id,
                    conversation.id,
                    incarnation,
                )
                .await?;
        }
        for target in targets {
            let target_key = target.device_key_id.to_string();
            let package = match readd_targets.get(&target.device_key_id) {
                // A re-add: a one-time package uploaded after the request, or nothing.
                Some(since) => {
                    Self::limit(
                        &self.one_time_claim_rate_limiter,
                        &target_key,
                        ONE_TIME_CLAIM_KEY,
                    )?;
                    let package = repos
                        .e2ee_repo
                        .claim_one_time_key_package(
                            target.device_key_id,
                            suite,
                            device.device_key_id,
                            conversation.id,
                            incarnation,
                            Some(*since),
                        )
                        .await?;
                    Some(package.ok_or_else(|| {
                        ServiceError::Validation(
                            "the device has not uploaded key packages since its re-add request"
                                .to_string(),
                        )
                    })?)
                }
                None => {
                    let one_time_allowed = matches!(
                        self.one_time_claim_rate_limiter.check_and_record(
                            &target_key,
                            ONE_TIME_CLAIM_KEY,
                            now
                        ),
                        RateLimitDecision::Allowed
                    );
                    let mut package = None;
                    if one_time_allowed {
                        package = repos
                            .e2ee_repo
                            .claim_one_time_key_package(
                                target.device_key_id,
                                suite,
                                device.device_key_id,
                                conversation.id,
                                incarnation,
                                None,
                            )
                            .await?;
                    }
                    if package.is_none() {
                        package = repos
                            .e2ee_repo
                            .claim_last_resort_key_package(
                                target.device_key_id,
                                suite,
                                device.device_key_id,
                                conversation.id,
                                incarnation,
                            )
                            .await?;
                    }
                    package
                }
            };
            if let Some(package) = package {
                claimed.push(ClaimedKeyPackageDto {
                    user_id: target.user_id,
                    principal_id: target.principal_id,
                    device_id: target.device_id,
                    key_package: package.key_package,
                    key_package_ref: package.key_package_ref,
                    last_resort: package.last_resort,
                });
            }
        }
        Self::commit_scope(scope).await?;
        info!(claimed = claimed.len(), device_key_id = %device.device_key_id, "claimed MLS key packages");
        Ok(claimed)
    }

    #[instrument(name = "e2ee.submit_commit", skip_all, fields(actor_id = %cmd.caller.actor.user_id, conversation_id = %cmd.conversation_id, incarnation = cmd.incarnation, epoch = cmd.epoch))]
    async fn submit_commit(&self, cmd: SubmitMlsCommitCmd) -> ServiceResult<MlsCommitDto> {
        let committer = session_device(self.repos.e2ee_repo.as_ref(), &cmd.caller).await?;
        let (conversation, _) = ensure_conversation_writable_with_standing(
            self.access_repos(),
            cmd.caller.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;
        ensure_encryptable(&conversation)?;
        if cmd.incarnation < 1 || cmd.epoch < 0 {
            return Err(ServiceError::Validation(
                "incarnation starts at 1 and epoch at 0".to_string(),
            ));
        }
        let commit_bytes = decode_b64("commit", &cmd.commit, MAX_COMMIT_BYTES)?;
        let header =
            read_private_message(self.wire.as_ref(), &commit_bytes, MlsContentType::Commit)?;
        let summary = parse_commit(&header.authenticated_data)?;
        let welcome = match &cmd.welcome {
            Some(text) => {
                let bytes = decode_b64("welcome", text, MAX_WELCOME_BYTES)?;
                let info = self
                    .wire
                    .read_welcome(&bytes)
                    .map_err(|e| ServiceError::Validation(e.0))?;
                Some((bytes, info.new_member_refs, info.cipher_suite))
            }
            None => None,
        };
        let now = Utc::now();

        let (scope, repos) = self.begin().await?;
        let accepted = match self
            .accept_commit(
                &repos,
                &cmd,
                &conversation,
                &committer,
                &header,
                &summary,
                commit_bytes,
                welcome,
                now,
            )
            .await
        {
            Ok(accepted) => accepted,
            Err(ServiceError::StaleEpoch { .. }) if self.db.is_some() => {
                // A race lost to a Commit that committed first: answer with the group as it is
                // now.
                drop(scope);
                let current = self.repos.e2ee_repo.current_group(conversation.id).await?;
                return Err(match current {
                    Some(group) => stale(&group),
                    None => ServiceError::StaleEpoch {
                        incarnation: 0,
                        epoch: 0,
                    },
                });
            }
            Err(e) => return Err(e),
        };
        Self::commit_scope(scope).await?;
        let commit = accepted.commit;
        info!(
            commit_id = %commit.id,
            conversation_id = %conversation.id,
            incarnation = commit.incarnation,
            epoch = commit.epoch,
            device_key_id = %committer.device_key_id,
            "accepted MLS Commit"
        );

        if cmd.epoch == 0 {
            let command_id = if commit.incarnation == 1 {
                AUDIT_ENABLE
            } else {
                AUDIT_REINCARNATE
            };
            self.audit(audit_entry(
                cmd.caller.actor.user_id,
                &conversation,
                command_id,
                serde_json::json!({
                    "conversation_id": conversation.id.to_string(),
                    "incarnation": commit.incarnation,
                    "device_key_id": committer.device_key_id.to_string(),
                }),
            ))
            .await;
        }

        let member_user_ids = self.member_ids(conversation.id).await?;
        self.observers.emit(WorkspaceEvent::MlsCommitAccepted {
            workspace_id: conversation.server_id,
            conversation_id: conversation.id,
            incarnation: commit.incarnation,
            epoch: commit.epoch,
            member_user_ids: member_user_ids.clone(),
        });
        for (user_id, device_id) in accepted.welcomed {
            self.observers.emit(WorkspaceEvent::MlsWelcomeAvailable {
                workspace_id: conversation.server_id,
                conversation_id: conversation.id,
                user_id,
                device_id,
                member_user_ids: vec![user_id],
            });
        }
        if accepted.membership_changed {
            self.observers.emit(WorkspaceEvent::MlsMembershipChanged {
                workspace_id: conversation.server_id,
                conversation_id: conversation.id,
                member_user_ids: member_user_ids.clone(),
            });
        }
        if accepted.newly_encrypted {
            self.observers.emit(WorkspaceEvent::ConversationUpdated {
                workspace_id: conversation.server_id,
                conversation_id: conversation.id,
                conversation_type: conversation.conversation_type.as_str().to_string(),
                is_private: conversation.is_private,
                member_user_ids,
            });
        }
        Ok(Self::commit_dto(commit))
    }

    async fn list_commits(&self, query: ListMlsCommitsQuery) -> ServiceResult<MlsCommitPageDto> {
        ensure_conversation_member_access(
            self.access_repos(),
            query.actor.user_id,
            query.server_id,
            query.conversation_id,
        )
        .await?;
        let limit = query.limit.unwrap_or(DEFAULT_COMMIT_PAGE);
        if !(1..=MAX_COMMIT_PAGE).contains(&limit) {
            return Err(ServiceError::Validation(format!(
                "limit must be 1 to {MAX_COMMIT_PAGE}"
            )));
        }
        let limit = limit.unsigned_abs();
        let mut commits = self
            .repos
            .e2ee_repo
            .list_commits(
                query.conversation_id,
                query.incarnation,
                query.after_epoch,
                limit + 1,
            )
            .await?;
        let has_more = commits.len() > limit as usize;
        commits.truncate(limit as usize);
        Ok(MlsCommitPageDto {
            commits: commits.into_iter().map(Self::commit_dto).collect(),
            has_more,
        })
    }

    async fn pending_welcomes(
        &self,
        query: ListMlsWelcomesQuery,
    ) -> ServiceResult<Vec<MlsWelcomeDto>> {
        let device = session_device(self.repos.e2ee_repo.as_ref(), &query.caller).await?;
        let limit = query.limit.unwrap_or(MAX_WELCOME_PAGE);
        if !(1..=MAX_WELCOME_PAGE).contains(&limit) {
            return Err(ServiceError::Validation(format!(
                "limit must be 1 to {MAX_WELCOME_PAGE}"
            )));
        }
        Ok(self
            .repos
            .e2ee_repo
            .pending_welcomes(device.device_key_id, query.after, limit.unsigned_abs())
            .await?
            .into_iter()
            .map(|w| MlsWelcomeDto {
                id: w.id,
                server_id: w.server_id,
                conversation_id: w.conversation_id,
                incarnation: w.incarnation,
                epoch: w.epoch,
                message: w.message,
                created_at: w.created_at,
                expires_at: w.expires_at,
            })
            .collect())
    }

    async fn ack_welcomes(&self, cmd: AckMlsWelcomesCmd) -> ServiceResult<u64> {
        let device = session_device(self.repos.e2ee_repo.as_ref(), &cmd.caller).await?;
        if cmd.ids.len() > MAX_ACKS {
            return Err(ServiceError::Validation(format!(
                "acknowledge at most {MAX_ACKS} Welcomes at once"
            )));
        }
        Ok(self
            .repos
            .e2ee_repo
            .ack_welcomes(device.device_key_id, &cmd.ids)
            .await?)
    }

    #[instrument(name = "e2ee.request_readd", skip_all, fields(actor_id = %cmd.caller.actor.user_id, conversation_id = %cmd.conversation_id))]
    async fn request_readd(&self, cmd: RequestMlsReaddCmd) -> ServiceResult<()> {
        let device = session_device(self.repos.e2ee_repo.as_ref(), &cmd.caller).await?;
        let conversation = ensure_conversation_member_access(
            self.access_repos(),
            cmd.caller.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;
        ensure_encryptable(&conversation)?;
        let device_key = rim_identity::PublicKey::parse(&device.public_key, "deviceKey")
            .map_err(ServiceError::internal)?;
        let statement = rim_identity::SignedStatement {
            payload: cmd.payload.clone(),
            signature: cmd.signature.clone(),
        };
        let request = rim_identity::verify_mls_readd(&device_key, &statement)
            .map_err(crate::util::statement_refused)?;
        if (request.issued_at - Utc::now()).abs() > READD_FRESHNESS {
            return Err(ServiceError::Validation(
                "issuedAt must be within 10 minutes of the server's clock".to_string(),
            ));
        }
        if Some(&request.principal_id) != device.principal_id.as_ref()
            || request.device_id != device.device_id
        {
            return Err(ServiceError::Validation(
                "the request names another device".to_string(),
            ));
        }
        if request.conversation_id != uuid::Uuid::from(conversation.id) {
            return Err(ServiceError::Validation(
                "the request names another conversation".to_string(),
            ));
        }
        let group = self
            .repos
            .e2ee_repo
            .current_group(conversation.id)
            .await?
            .ok_or_else(|| {
                ServiceError::Validation("the conversation has no MLS group".to_string())
            })?;
        let incarnation = i64::from(request.incarnation);
        if incarnation < i64::from(group.incarnation) {
            return Err(stale(&group));
        }
        if incarnation > i64::from(group.incarnation) {
            return Err(ServiceError::Validation(format!(
                "incarnation {incarnation} does not exist"
            )));
        }
        let devices = self
            .repos
            .e2ee_repo
            .group_devices(conversation.id, group.incarnation)
            .await?;
        let Some(member) = devices
            .iter()
            .find(|d| d.device.device_key_id == device.device_key_id)
        else {
            return Err(ServiceError::Forbidden(
                "this device is not in the conversation's group".to_string(),
            ));
        };
        // A device holding state reached at least the epoch it was last added at, so a request
        // made before its latest add is refused.
        if let Some(last_epoch) = request.last_epoch {
            let last_epoch = i64::try_from(last_epoch).unwrap_or(i64::MAX);
            if last_epoch > group.epoch {
                return Err(ServiceError::Validation(
                    "lastEpoch is past the group's epoch".to_string(),
                ));
            }
            if last_epoch < member.added_epoch {
                return Err(ServiceError::Validation(
                    "lastEpoch is before the epoch this device was added at".to_string(),
                ));
            }
        }
        Self::limit(
            &self.readd_rate_limiter,
            &format!("{}:{}", device.device_key_id, conversation.id),
            READD_KEY,
        )?;
        match self
            .repos
            .e2ee_repo
            .insert_readd_request(NewMlsReaddRequest {
                conversation_id: conversation.id,
                incarnation: group.incarnation,
                device_key_id: device.device_key_id,
                payload: cmd.payload,
                signature: cmd.signature,
                nonce: request.nonce.to_vec(),
            })
            .await
        {
            Ok(_) => {}
            Err(e) if e.kind() == RepoErrorKind::Conflict => {
                return Err(ServiceError::Conflict(
                    "this re-add request was already sent".to_string(),
                ));
            }
            Err(e) => return Err(e.into()),
        }
        info!(device_key_id = %device.device_key_id, conversation_id = %conversation.id, incarnation = group.incarnation, "MLS re-add requested");
        self.emit_membership_changed(&conversation).await;
        Ok(())
    }

    async fn group(&self, query: GetMlsGroupQuery) -> ServiceResult<Option<MlsGroupDto>> {
        let conversation = ensure_conversation_member_access(
            self.access_repos(),
            query.actor.user_id,
            query.server_id,
            query.conversation_id,
        )
        .await?;
        let Some(group) = self.repos.e2ee_repo.current_group(conversation.id).await? else {
            return Ok(None);
        };
        let state = GroupState::load(self.repos.e2ee_repo.as_ref(), group, Utc::now()).await?;
        let device_dto = |d: &MlsGroupDevice| MlsDeviceDto {
            user_id: d.device.user_id,
            principal_id: d.device.principal_id.clone(),
            device_id: d.device.device_id,
            added_epoch: d.added_epoch,
        };
        Ok(Some(MlsGroupDto {
            group_id: state.group.group_id.clone(),
            incarnation: state.group.incarnation,
            cipher_suite: state.group.cipher_suite,
            epoch: state.group.epoch,
            devices: state.devices.iter().map(device_dto).collect(),
            pending_adds: state
                .pending_adds()
                .map(|d| MlsPendingDeviceDto {
                    user_id: d.device.user_id,
                    principal_id: d.device.principal_id.clone(),
                    device_id: d.device.device_id,
                    has_key_package: d.has_key_package,
                })
                .collect(),
            pending_removes: state.pending_removes().map(device_dto).collect(),
            readd_requests: state
                .readd_requests
                .iter()
                .map(|r| MlsReaddRequestDto {
                    principal_id: r.device.principal_id.clone(),
                    device_id: r.device.device_id,
                    payload: r.payload.clone(),
                    signature: r.signature.clone(),
                    created_at: r.created_at,
                })
                .collect(),
        }))
    }

    async fn principal_devices(
        &self,
        query: PrincipalDevicesQuery,
    ) -> ServiceResult<Vec<PrincipalDevicesDto>> {
        if query.principal_ids.len() > MAX_PRINCIPALS {
            return Err(ServiceError::Validation(format!(
                "ask for at most {MAX_PRINCIPALS} principals"
            )));
        }
        let mut seen = HashSet::new();
        let principal_ids: Vec<String> = query
            .principal_ids
            .into_iter()
            .filter(|p| seen.insert(p.clone()))
            .collect();
        let visible: HashMap<String, UserId> = self
            .repos
            .e2ee_repo
            .principals_visible_to(query.actor.user_id, &principal_ids)
            .await?
            .into_iter()
            .collect();
        let since = active_since(Utc::now());
        let mut result = Vec::with_capacity(principal_ids.len());
        for principal_id in principal_ids {
            let devices = match visible.get(&principal_id) {
                Some(user_id) => {
                    let active: HashSet<uuid::Uuid> = self
                        .repos
                        .e2ee_repo
                        .mls_active_device_ids(*user_id, since)
                        .await?
                        .into_iter()
                        .collect();
                    self.repos
                        .key_identity_repo
                        .list_devices(*user_id)
                        .await?
                        .into_iter()
                        .map(|d| PrincipalDeviceDto {
                            mls_active: active.contains(&d.device_id),
                            device_id: d.device_id,
                            name: d.name,
                            cert_payload: d.cert_payload,
                            cert_signature: d.cert_signature,
                            revoked_at: d.revoked_at,
                            revocation_payload: d.revocation_payload,
                            revocation_signature: d.revocation_signature,
                        })
                        .collect()
                }
                None => Vec::new(),
            };
            result.push(PrincipalDevicesDto {
                principal_id,
                devices,
            });
        }
        Ok(result)
    }

    #[instrument(name = "e2ee.report_encrypted_message", skip_all, fields(actor_id = %cmd.actor.user_id, message_id = %cmd.message_id))]
    async fn report_encrypted_message(&self, cmd: ReportEncryptedMessageCmd) -> ServiceResult<()> {
        let reason = cmd.reason.trim();
        if reason.is_empty() {
            return Err(ServiceError::Validation(
                "a report needs a reason".to_string(),
            ));
        }
        if reason.chars().count() > MAX_REPORT_REASON_CHARS {
            return Err(ServiceError::Validation(format!(
                "reason must be at most {MAX_REPORT_REASON_CHARS} characters"
            )));
        }
        let conversation = ensure_conversation_member_access(
            self.access_repos(),
            cmd.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;
        let message = self
            .repos
            .message_repo
            .get_by_id(cmd.message_id)
            .await?
            .filter(|m| m.conversation_id == conversation.id)
            .ok_or(ServiceError::NotFound)?;
        if message.encryption.is_none() {
            return Err(ServiceError::Validation(
                "this message is not encrypted; report it with reportMessage".to_string(),
            ));
        }
        let content = decode_b64("content", &cmd.content, MAX_CONTENT_BYTES)?;
        let franking_key = decode_fixed::<FRANKING_LEN>("frankingKey", &cmd.franking_key)?;
        let receipt = decode_fixed::<FRANKING_LEN>("receipt", &cmd.receipt)?;
        let sent_at_us: i64 = cmd.sent_at_us.parse().map_err(|_| {
            ServiceError::Validation("sentAtUs must be decimal microseconds".to_string())
        })?;
        let mismatch =
            || ServiceError::Validation("the report does not match the message".to_string());
        let franking_keys = self.franking_keys.as_ref().ok_or_else(|| {
            ServiceError::internal(anyhow::anyhow!("franking keys are not configured"))
        })?;
        let server_key = franking_keys
            .verifying(cmd.receipt_key_id)
            .ok_or_else(mismatch)?;
        let fields = ReceiptFields {
            message_id: message.id.into(),
            conversation_id: conversation.id.into(),
            sender_user_id: cmd.sender_user_id.into(),
            sender_device_id: cmd.sender_device_id,
            sent_at_us,
        };
        let c = commitment(&franking_key, &content);
        // The receipt alone proves only that someone holding a franking key signed it, so the
        // report must also name a version the server recorded: the commitment, send time and
        // sender device of the message as sent or as any later edit. The row as it is now also
        // counts, for a message whose versions were recorded without its sender device.
        let revisions = self.repos.e2ee_repo.franking_revisions(message.id).await?;
        let stored = message
            .encryption
            .as_ref()
            .map(|e| e.franking_commitment.as_slice())
            .unwrap_or_default();
        let row_device = match message
            .encryption
            .as_ref()
            .and_then(|e| e.sender_device_key_id)
        {
            Some(id) => self
                .repos
                .e2ee_repo
                .get_device(id)
                .await?
                .map(|d| d.device_id),
            None => None,
        };
        // Every version is compared, without stopping at a match.
        let mut recorded = bool::from(stored.ct_eq(&c[..]))
            & (sent_at_us == crate::e2ee::franking::sent_at_us(message.sent_at))
            & row_device.is_none_or(|d| d == cmd.sender_device_id);
        for revision in &revisions {
            recorded |= bool::from(revision.franking_commitment.ct_eq(&c[..]))
                & (revision.sent_at_us == sent_at_us)
                & (revision.sender_device_id == cmd.sender_device_id);
        }
        if cmd.sender_user_id != message.sent_by
            || !verify_receipt(server_key.as_ref(), &fields, &c, &receipt)
            || !recorded
        {
            return Err(mismatch());
        }

        let report = match self
            .repos
            .moderation_repo
            .create_report(NewMessageReport {
                server_id: conversation.server_id,
                conversation_id: conversation.id,
                message_id: message.id,
                reporter_user_id: cmd.actor.user_id,
                reason: reason.to_string(),
                franked: Some(NewFrankedReport {
                    content,
                    sender_device_id: cmd.sender_device_id,
                }),
            })
            .await
        {
            Ok(report) => report,
            Err(e) if e.kind() == RepoErrorKind::Conflict => {
                return Err(ServiceError::Conflict(
                    "you already reported this message".to_string(),
                ));
            }
            Err(e) => return Err(e.into()),
        };
        info!(report_id = %report.id, message_id = %message.id, "franked report verified");
        self.audit(audit_entry(
            cmd.actor.user_id,
            &conversation,
            AUDIT_REPORT,
            serde_json::json!({
                "report_id": report.id.to_string(),
                "conversation_id": conversation.id.to_string(),
                "message_id": message.id.to_string(),
                "author_id": message.sent_by.to_string(),
            }),
        ))
        .await;
        Ok(())
    }

    async fn devices_changed(&self, user_id: UserId) -> ServiceResult<()> {
        let ids = self
            .repos
            .e2ee_repo
            .grouped_conversations_of_user(user_id)
            .await?;
        for conversation in self.repos.conversation_repo.get_by_ids(&ids).await? {
            self.emit_membership_changed(&conversation).await;
        }
        Ok(())
    }

    async fn purge(&self) -> ServiceResult<MlsPurged> {
        Ok(self
            .repos
            .e2ee_repo
            .purge(MlsRetention {
                commits: Duration::days(90),
                claimed_key_packages: Duration::days(7),
                readd_requests: READD_REQUEST_TTL,
                readd_nonces: READD_NONCE_TTL,
                franked_content: Duration::days(90),
            })
            .await?)
    }
}
