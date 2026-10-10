//! The host side: joins through a relay, delegations and subscriptions, presence relays report,
//! and revocations relays and apps bring.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use rim_auth::session_token::{generate_challenge_nonce, hash_token};
use rim_core::auth::repository::{InviteRepository, SessionRepository};
use rim_core::e2ee::repository::E2eeRepository;
use rim_core::host::model::{DelegationStored, NewDelegation, UserKind};
use rim_core::host::repository::{DelegationRepository, HostRepository};
use rim_core::key_identity::model::{DeviceKey, DeviceKeyId};
use rim_core::key_identity::repository::{KeyChallengeRepository, KeyIdentityRepository};
use rim_core::moderation::repository::ModerationRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_identity::{
    Delegation, DelegationError, DelegationParties, KeyState, SignedStatement,
    UnverifiedDelegation, UnverifiedLogin, UnverifiedRevocation,
};
use rim_s2s::SharedIdentity;
use rim_s2s::revocation::{RevocationAckFields, sign_revocation_ack};
use rim_services::audit::dto::{AuditLogEntryDto, AuditOutcome};
use rim_services::audit::service::AuditLogService;
use rim_services::auth::command::JoinServerByInviteCmd;
use rim_services::auth::service::AuthService;
use rim_services::command::verb::TypedVerb;
use rim_services::e2ee::service::E2eeService;
use rim_services::host::service::{DelegationCheck, HostService};
use rim_services::presence::PresenceService;
use rim_services::relay::dto::{IdentityDevice, IdentityDocument, SignedJson};
use rim_services::remote_workspace::dto::{
    CancelEntry, JoinAnswer, JoinChallengeDto, JoinRequest, PresenceUpdate, RemoteWorkspaceCleanup,
    SubscriptionResult,
};
use rim_services::remote_workspace::service::WorkspaceHostService;
use rim_services::s2s::service::PeerService;
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::host::DefaultHostService;
use crate::relay::identity::device_key;
use crate::remote_workspace::{MAX_ACCEPTED_LIFETIME, MAX_DELEGATIONS_PER_REQUEST};

/// How long a join challenge stays valid.
const JOIN_CHALLENGE_TTL: chrono::Duration = chrono::Duration::seconds(120);
/// `initiator` of the audit rows a host writes for remote members.
const AUDIT_INITIATOR: &str = "s2s";

#[derive(Clone)]
pub struct WorkspaceHostRepos {
    pub host_repo: Arc<dyn HostRepository>,
    pub delegation_repo: Arc<dyn DelegationRepository>,
    pub key_identity_repo: Arc<dyn KeyIdentityRepository>,
    pub key_challenge_repo: Arc<dyn KeyChallengeRepository>,
    pub invite_repo: Arc<dyn InviteRepository>,
    pub moderation_repo: Arc<dyn ModerationRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub session_repo: Arc<dyn SessionRepository>,
    pub e2ee_repo: Arc<dyn E2eeRepository>,
}

/// Services built after this one, set once the provider exists.
#[derive(Default)]
struct Later {
    auth: OnceLock<Arc<dyn AuthService>>,
    presence: OnceLock<Arc<dyn PresenceService>>,
    e2ee: OnceLock<Arc<dyn E2eeService>>,
    audit: OnceLock<Arc<dyn AuditLogService>>,
}

pub struct DefaultWorkspaceHost {
    repos: WorkspaceHostRepos,
    identity: SharedIdentity,
    peers: Arc<dyn PeerService>,
    host: Arc<DefaultHostService>,
    /// Active delegations one relay may hold here.
    delegations_per_relay: i64,
    later: Later,
}

/// Whether a relay holds an active delegation of a device, from the stored delegations.
pub struct RepoDelegationCheck {
    delegation_repo: Arc<dyn DelegationRepository>,
}

impl RepoDelegationCheck {
    pub fn new(delegation_repo: Arc<dyn DelegationRepository>) -> Self {
        Self { delegation_repo }
    }
}

#[async_trait]
impl DelegationCheck for RepoDelegationCheck {
    async fn delegated(
        &self,
        _user_id: UserId,
        device_key_id: DeviceKeyId,
        relay_server_id: &str,
    ) -> ServiceResult<bool> {
        Ok(self
            .delegation_repo
            .device_delegated(device_key_id, relay_server_id, Utc::now())
            .await?)
    }
}

fn statement(s: &SignedJson) -> SignedStatement {
    SignedStatement {
        payload: s.payload.clone(),
        signature: s.signature.clone(),
    }
}

/// The device id a certificate names, read before it is verified.
fn certificate_device(certificate: &SignedJson) -> ServiceResult<Uuid> {
    serde_json::from_str::<serde_json::Value>(&certificate.payload)
        .ok()
        .and_then(|v| v.get("deviceId")?.as_str()?.parse().ok())
        .ok_or_else(|| ServiceError::Validation("a certificate names no device".to_string()))
}

/// When the host stops honouring a delegation it accepts now.
fn accepted_expiry(delegation: &Delegation, now: DateTime<Utc>) -> DateTime<Utc> {
    delegation.expires_at.min(now + MAX_ACCEPTED_LIFETIME)
}

impl DefaultWorkspaceHost {
    pub fn new(
        repos: WorkspaceHostRepos,
        identity: SharedIdentity,
        peers: Arc<dyn PeerService>,
        host: Arc<DefaultHostService>,
        delegations_per_relay: i64,
    ) -> Self {
        Self {
            repos,
            identity,
            peers,
            host,
            delegations_per_relay,
            later: Later::default(),
        }
    }

    /// Wire the services built after this one: joins by invite, presence, device changes in
    /// encrypted groups, and the audit log.
    pub fn set_services(
        &self,
        auth: Arc<dyn AuthService>,
        presence: Arc<dyn PresenceService>,
        e2ee: Arc<dyn E2eeService>,
        audit: Arc<dyn AuditLogService>,
    ) {
        let _ = self.later.auth.set(auth);
        let _ = self.later.presence.set(presence);
        let _ = self.later.e2ee.set(e2ee);
        let _ = self.later.audit.set(audit);
    }

    fn own_id(&self) -> String {
        self.identity.current().server_id().to_string()
    }

    /// Record an event about a member in a workspace: an audit line, and a `command_audit_log`
    /// row once the audit log is wired.
    async fn audit(
        &self,
        event: &'static str,
        user_id: UserId,
        workspace_id: ServerId,
        args: serde_json::Value,
    ) {
        info!(
            target: "audit",
            event,
            user_id = %user_id,
            workspace_id = %workspace_id,
            args = %args,
        );
        let Some(audit) = self.later.audit.get() else {
            return;
        };
        let row = AuditLogEntryDto {
            id: Uuid::now_v7(),
            actor_user_id: user_id.into(),
            server_id: workspace_id.into(),
            command_id: event.to_string(),
            initiator: AUDIT_INITIATOR.to_string(),
            args_redacted: args,
            sensitivity: "restricted".to_string(),
            dangerous: false,
            confirmation_kind: "none".to_string(),
            typed_verb: Some(TypedVerb::Write),
            outcome: AuditOutcome::Success,
            error_kind: None,
            capability_check_outcome: None,
            confirmation_outcome: None,
            parent_audit_id: None,
            created_at: Utc::now(),
        };
        if let Err(e) = audit.write_entry(row).await {
            warn!(error = %e, event, "could not write an audit row");
        }
    }

    /// Hashes of the presenting relay's keys a delegation may name.
    async fn relay_key_hashes(&self, relay_server_id: &str) -> ServiceResult<Vec<String>> {
        let log: Vec<SignedStatement> = self
            .peers
            .pinned_key_log(relay_server_id)
            .await?
            .iter()
            .map(statement)
            .collect();
        if log.is_empty() {
            return Ok(Vec::new());
        }
        rim_s2s::delegable_key_hashes(&log).map_err(|e| {
            ServiceError::internal(anyhow::anyhow!("a pinned key log does not verify: {e}"))
        })
    }

    /// The join's identity as a document to verify and store. Devices this host already
    /// knows keep whether they are active for encryption; new ones start inactive until the
    /// relay pushes the full identity.
    async fn join_document(
        &self,
        principal_id: &str,
        request: &JoinRequest,
    ) -> ServiceResult<IdentityDocument> {
        let active: HashSet<Uuid> = match self.repos.host_repo.find_user(principal_id).await? {
            Some((user_id, UserKind::Remote)) => self
                .repos
                .e2ee_repo
                .mls_active_device_ids(user_id, crate::e2ee::active_since(Utc::now()))
                .await?
                .into_iter()
                .collect(),
            _ => HashSet::new(),
        };
        let mut devices = Vec::with_capacity(request.devices.len());
        for device in &request.devices {
            let device_id = certificate_device(&device.certificate)?;
            devices.push(IdentityDevice {
                device_id,
                certificate: device.certificate.clone(),
                revocation: device.revocation.clone(),
                mls_active: active.contains(&device_id),
            });
        }
        Ok(IdentityDocument {
            principal_id: principal_id.to_string(),
            key_log: request.key_log.clone(),
            devices,
            card: request.card.clone(),
        })
    }

    /// A user's device as stored here, if it is not revoked.
    async fn live_device(
        &self,
        user_id: UserId,
        device_id: Uuid,
    ) -> ServiceResult<Option<DeviceKey>> {
        Ok(self
            .repos
            .key_identity_repo
            .get_device(user_id, device_id)
            .await?
            .filter(|d| d.revoked_at.is_none()))
    }

    /// Check a delegation's rules and signature under its stored device, then store it.
    #[allow(clippy::too_many_arguments)]
    async fn accept(
        &self,
        peer_server_id: &str,
        relay_key_hashes: &[String],
        unverified: UnverifiedDelegation,
        user_id: UserId,
        kind: UserKind,
        device: &DeviceKey,
        statement: &SignedJson,
        now: DateTime<Utc>,
    ) -> Result<DelegationStored, &'static str> {
        let own_id = self.own_id();
        let key = device_key(&device.public_key).map_err(|_| "invalid_delegation")?;
        let delegation = unverified
            .verify(
                &key,
                &DelegationParties {
                    host: &own_id,
                    relay: peer_server_id,
                    relay_key_hashes,
                },
                now,
            )
            .map_err(|e: DelegationError| e.reason())?;
        let workspace_id = ServerId::from(delegation.workspace_id);
        if workspace_id == self.host.direct_space_id() {
            return Err("not_member");
        }
        let internal = |e: ServiceError| {
            warn!(error = %e, "a delegation check failed");
            "internal"
        };
        if self
            .repos
            .server_member_repo
            .get_member(workspace_id, user_id)
            .await
            .map_err(|e| internal(e.into()))?
            .is_none()
        {
            return Err("not_member");
        }
        if crate::util::ensure_not_banned(
            self.repos.moderation_repo.as_ref(),
            workspace_id,
            user_id,
        )
        .await
        .is_err()
        {
            return Err("banned");
        }
        let active = self
            .repos
            .delegation_repo
            .count_active_for_relay(peer_server_id, now)
            .await
            .map_err(|e| internal(e.into()))?;
        if active >= self.delegations_per_relay {
            warn!(peer_server_id, active, "a relay is at its delegation limit");
            return Err("limited");
        }
        let stored = self
            .repos
            .delegation_repo
            .store(
                NewDelegation {
                    workspace_id,
                    user_id,
                    device_key_id: device.id,
                    relay_server_id: peer_server_id.to_string(),
                    payload: statement.payload.clone(),
                    signature: statement.signature.clone(),
                    expires_at: accepted_expiry(&delegation, now),
                },
                now,
            )
            .await
            .map_err(|e| internal(e.into()))?;
        if kind == UserKind::Remote
            && let Err(e) = self
                .repos
                .host_repo
                .update_remote_user(user_id, None, Some(peer_server_id.to_string()))
                .await
        {
            warn!(error = %e, user_id = %user_id, "could not record a remote user's relay");
        }
        self.audit(
            "host.delegation_accepted",
            user_id,
            workspace_id,
            serde_json::json!({
                "relay_server_id": peer_server_id,
                "device_key_id": device.id.to_string(),
            }),
        )
        .await;
        Ok(stored)
    }

    /// Verify a revocation against the stored key log: under any identity key since the
    /// device's certificate, or the device's own key.
    async fn verify_revocation(
        &self,
        user_id: UserId,
        device: &DeviceKey,
        unverified: &UnverifiedRevocation,
    ) -> ServiceResult<()> {
        let log: Vec<SignedStatement> = self
            .repos
            .key_identity_repo
            .list_key_log(user_id)
            .await?
            .into_iter()
            .map(|e| SignedStatement {
                payload: e.payload,
                signature: e.signature,
            })
            .collect();
        let (first, rest) = log
            .split_first()
            .ok_or_else(|| ServiceError::Validation("no key log is stored".to_string()))?;
        let mut states =
            vec![KeyState::from_inception(first).map_err(crate::util::statement_refused)?];
        for entry in rest {
            let last = states.last().cloned().ok_or(ServiceError::NotFound)?;
            states.push(last.apply(entry).map_err(crate::util::statement_refused)?.0);
        }
        let key = device_key(&device.public_key)?;
        let from = usize::try_from(device.key_seq).unwrap_or(0);
        let verified = states
            .iter()
            .skip(from)
            .rev()
            .any(|state| unverified.clone().verify(state, &key).is_ok());
        if verified {
            Ok(())
        } else {
            Err(ServiceError::Validation(
                "the revocation does not verify".to_string(),
            ))
        }
    }
}

#[async_trait]
impl WorkspaceHostService for DefaultWorkspaceHost {
    async fn join_challenge(&self, peer_server_id: String) -> ServiceResult<JoinChallengeDto> {
        tracing::debug!(peer_server_id = %peer_server_id, "issuing a join challenge");
        let now = Utc::now();
        if let Err(e) = self.repos.key_challenge_repo.delete_expired(now).await {
            warn!(error = %e, "could not delete expired key challenges");
        }
        let challenge = self
            .repos
            .key_challenge_repo
            .create(&generate_challenge_nonce(), now + JOIN_CHALLENGE_TTL)
            .await?;
        Ok(JoinChallengeDto {
            challenge_id: challenge.id.into(),
            nonce: challenge.nonce,
            expires_at: challenge.expires_at,
        })
    }

    #[instrument(name = "host.join", skip_all, fields(peer_server_id = %peer_server_id))]
    async fn join(
        &self,
        peer_server_id: String,
        request: JoinRequest,
    ) -> ServiceResult<JoinAnswer> {
        let now = Utc::now();
        let own_id = self.own_id();
        let delegation = UnverifiedDelegation::read(&statement(&request.delegation))
            .map_err(|e| ServiceError::Validation(format!("delegation: {e}")))?;
        let fields = delegation.fields().clone();
        let workspace_id = ServerId::from(fields.workspace_id);
        if workspace_id == self.host.direct_space_id() {
            return Err(ServiceError::Validation(
                "the direct space is joined through conversations".to_string(),
            ));
        }
        if fields.relay != peer_server_id {
            return Err(ServiceError::Forbidden(
                "the delegation names another relay".to_string(),
            ));
        }

        // Everything that needs no storage first: the identity, the proof and the invite.
        let document = self.join_document(&fields.principal_id, &request).await?;
        let verified =
            crate::relay::identity::verify_identity(&document).map_err(ServiceError::Validation)?;
        let login = UnverifiedLogin::read(&statement(&request.proof))
            .map_err(crate::util::statement_refused)?;
        if login.principal_id != fields.principal_id {
            return Err(ServiceError::Validation(
                "the proof is another principal's".to_string(),
            ));
        }
        let proof_device = verified
            .devices
            .iter()
            .find(|d| d.cert.device_id == login.device_id && d.revocation.is_none())
            .ok_or_else(|| {
                ServiceError::Validation("the proof's device is unknown or revoked".to_string())
            })?;
        let proof = login
            .verify(&proof_device.cert.device_key)
            .map_err(crate::util::statement_refused)?;
        if proof.audience != own_id {
            return Err(ServiceError::Validation(
                "the proof names another server as its audience".to_string(),
            ));
        }
        if let Some(token) = &request.invite_token {
            let invite = self
                .repos
                .invite_repo
                .get_by_token_hash(&hash_token(token))
                .await?
                .ok_or(ServiceError::NotFound)?;
            if invite.server_id != workspace_id {
                return Err(ServiceError::Validation(
                    "the invite is for another workspace than the delegation".to_string(),
                ));
            }
        }
        if !self
            .repos
            .key_challenge_repo
            .consume(proof.challenge_id.into(), &proof.nonce)
            .await?
        {
            return Err(ServiceError::Validation(
                "the challenge is unknown, already used or expired".to_string(),
            ));
        }

        // The person as a user here: a remote user created or updated, or a local account.
        self.host.store_identity(&document, &peer_server_id).await?;
        let (user_id, kind) = self
            .repos
            .host_repo
            .find_user(&fields.principal_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let device = self
            .live_device(user_id, fields.device_id)
            .await?
            .ok_or_else(|| {
                ServiceError::Validation(
                    "the delegation's device is unknown or revoked".to_string(),
                )
            })?;
        if kind == UserKind::Local {
            // A local account's own devices decide, not the ones the relay sent.
            let key = device_key(
                &self
                    .live_device(user_id, proof.device_id)
                    .await?
                    .ok_or_else(|| {
                        ServiceError::Validation(
                            "the proof's device is unknown or revoked".to_string(),
                        )
                    })?
                    .public_key,
            )?;
            UnverifiedLogin::read(&statement(&request.proof))
                .and_then(|l| l.verify(&key))
                .map_err(crate::util::statement_refused)?;
        }

        match &request.invite_token {
            Some(token) => {
                let auth = self.later.auth.get().ok_or_else(|| {
                    ServiceError::internal(anyhow::anyhow!("joins are not wired"))
                })?;
                match auth
                    .join_server_by_invite(JoinServerByInviteCmd {
                        actor: Actor::new(user_id),
                        invite_token: token.clone(),
                    })
                    .await
                {
                    Ok(_) | Err(ServiceError::Conflict(_)) => {}
                    Err(e) => return Err(e),
                }
            }
            None => {
                crate::util::ensure_not_banned(
                    self.repos.moderation_repo.as_ref(),
                    workspace_id,
                    user_id,
                )
                .await?;
            }
        }
        let member = self
            .repos
            .server_member_repo
            .get_member(workspace_id, user_id)
            .await?
            .ok_or_else(|| ServiceError::Forbidden("not a member of this workspace".to_string()))?;

        let hashes = self.relay_key_hashes(&peer_server_id).await?;
        self.accept(
            &peer_server_id,
            &hashes,
            delegation,
            user_id,
            kind,
            &device,
            &request.delegation,
            now,
        )
        .await
        .map_err(|reason| match reason {
            "internal" => {
                ServiceError::internal(anyhow::anyhow!("a delegation could not be stored"))
            }
            "limited" => ServiceError::RateLimited {
                retry_after_seconds: 60,
            },
            reason => ServiceError::Validation(format!("delegation refused: {reason}")),
        })?;

        let streams = vec!["w".to_string(), format!("u:{}", fields.principal_id)];
        let stream_heads: BTreeMap<String, i64> = self
            .host
            .stream_heads(Actor::new(user_id), workspace_id, streams)
            .await?
            .into_iter()
            .map(|h| (h.stream, h.seq))
            .collect();
        self.audit(
            "host.remote_join",
            user_id,
            workspace_id,
            serde_json::json!({
                "relay_server_id": peer_server_id,
                "invite": request.invite_token.is_some(),
            }),
        )
        .await;
        Ok(JoinAnswer {
            workspace_id: workspace_id.into(),
            user_id: user_id.into(),
            role: member.member_role.as_str().to_string(),
            stream_heads,
        })
    }

    #[instrument(name = "host.subscribe", skip_all, fields(peer_server_id = %peer_server_id, delegations = delegations.len()))]
    async fn subscribe(
        &self,
        peer_server_id: String,
        delegations: Vec<SignedJson>,
    ) -> ServiceResult<Vec<SubscriptionResult>> {
        if delegations.len() > MAX_DELEGATIONS_PER_REQUEST {
            return Err(ServiceError::Validation(format!(
                "present at most {MAX_DELEGATIONS_PER_REQUEST} delegations"
            )));
        }
        let now = Utc::now();
        let hashes = self.relay_key_hashes(&peer_server_id).await?;
        let mut refreshed = HashSet::new();
        let mut results = Vec::with_capacity(delegations.len());
        for presented in &delegations {
            let Ok(unverified) = UnverifiedDelegation::read(&statement(presented)) else {
                results.push(SubscriptionResult::refused(None, "invalid_delegation"));
                continue;
            };
            let fields = unverified.fields().clone();
            let workspace = Some(fields.workspace_id);
            let mut found = self.repos.host_repo.find_user(&fields.principal_id).await?;
            let mut device = match found {
                Some((user_id, _)) => {
                    self.repos
                        .key_identity_repo
                        .get_device(user_id, fields.device_id)
                        .await?
                }
                None => None,
            };
            // A device this host has not seen yet: fetch the person's identity once from the
            // relay presenting it.
            if device.is_none() && refreshed.insert(fields.principal_id.clone()) {
                self.host
                    .refresh_identity(fields.principal_id.clone(), Some(peer_server_id.clone()))
                    .await?;
                found = self.repos.host_repo.find_user(&fields.principal_id).await?;
                if let Some((user_id, _)) = found {
                    device = self
                        .repos
                        .key_identity_repo
                        .get_device(user_id, fields.device_id)
                        .await?;
                }
            }
            let (Some((user_id, kind)), Some(device)) = (found, device) else {
                results.push(SubscriptionResult::refused(workspace, "unknown_device"));
                continue;
            };
            if device.revoked_at.is_some() {
                results.push(SubscriptionResult::refused(workspace, "revoked_device"));
                continue;
            }
            match self
                .accept(
                    &peer_server_id,
                    &hashes,
                    unverified,
                    user_id,
                    kind,
                    &device,
                    presented,
                    now,
                )
                .await
            {
                Ok(stored) => results.push(SubscriptionResult::active(
                    fields.workspace_id,
                    stored.expires_at(),
                )),
                Err(reason) => results.push(SubscriptionResult::refused(workspace, reason)),
            }
        }
        Ok(results)
    }

    #[instrument(name = "host.cancel_subscriptions", skip_all, fields(peer_server_id = %peer_server_id))]
    async fn cancel(
        &self,
        peer_server_id: String,
        entries: Vec<CancelEntry>,
    ) -> ServiceResult<u64> {
        if entries.len() > MAX_DELEGATIONS_PER_REQUEST {
            return Err(ServiceError::Validation(format!(
                "cancel at most {MAX_DELEGATIONS_PER_REQUEST} subscriptions"
            )));
        }
        let mut ended = 0;
        for entry in entries {
            if let Some((user_id, _)) = self.repos.host_repo.find_user(&entry.principal_id).await? {
                ended += self
                    .repos
                    .delegation_repo
                    .end_for_user(entry.workspace_id.into(), user_id, Some(&peer_server_id))
                    .await?;
            }
        }
        info!(ended, "a relay cancelled subscriptions");
        Ok(ended)
    }

    async fn presence(
        &self,
        peer_server_id: String,
        updates: Vec<PresenceUpdate>,
    ) -> ServiceResult<u64> {
        if updates.len() > MAX_DELEGATIONS_PER_REQUEST {
            return Err(ServiceError::Validation(format!(
                "report at most {MAX_DELEGATIONS_PER_REQUEST} updates"
            )));
        }
        let Some(presence) = self.later.presence.get() else {
            return Ok(0);
        };
        let now = Utc::now();
        let mut applied = 0;
        for update in updates {
            let Some((user_id, _)) = self.repos.host_repo.find_user(&update.principal_id).await?
            else {
                continue;
            };
            let workspaces = self
                .repos
                .delegation_repo
                .delegated_workspaces(user_id, &peer_server_id, now)
                .await?;
            if workspaces.is_empty() {
                continue;
            }
            presence.set_remote(user_id, workspaces, update.online);
            applied += 1;
        }
        Ok(applied)
    }

    #[instrument(name = "host.apply_revocation", skip_all)]
    async fn apply_revocation(
        &self,
        peer_server_id: Option<String>,
        principal_id: Option<String>,
        revocation: SignedJson,
    ) -> ServiceResult<SignedJson> {
        let unverified = UnverifiedRevocation::read(&statement(&revocation))
            .map_err(crate::util::statement_refused)?;
        if principal_id
            .as_deref()
            .is_some_and(|p| p != unverified.principal_id)
        {
            return Err(ServiceError::Validation(
                "the revocation is another principal's".to_string(),
            ));
        }
        let user_id = self
            .repos
            .key_identity_repo
            .find_user_by_principal_id(&unverified.principal_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let mut device = self
            .repos
            .key_identity_repo
            .get_device(user_id, unverified.device_id)
            .await?;
        if device.is_none()
            && let Some(peer) = &peer_server_id
        {
            self.host
                .refresh_identity(unverified.principal_id.clone(), Some(peer.clone()))
                .await?;
            device = self
                .repos
                .key_identity_repo
                .get_device(user_id, unverified.device_id)
                .await?;
        }
        let device = device.ok_or(ServiceError::NotFound)?;
        self.verify_revocation(user_id, &device, &unverified)
            .await?;

        let newly = self
            .repos
            .key_identity_repo
            .revoke_device(device.id, &revocation.payload, &revocation.signature)
            .await?;
        let delegations = self.repos.delegation_repo.end_for_device(device.id).await?;
        let sessions = self
            .repos
            .session_repo
            .revoke_all_for_device(device.id)
            .await?;
        if newly
            && let Some(e2ee) = self.later.e2ee.get()
            && let Err(e) = e2ee.devices_changed(user_id).await
        {
            warn!(error = %e, user_id = %user_id, "could not announce a revoked device to encrypted conversations");
        }
        let applied_at = self
            .repos
            .key_identity_repo
            .get_device(user_id, unverified.device_id)
            .await?
            .and_then(|d| d.revoked_at)
            .unwrap_or_else(Utc::now);
        if newly {
            info!(
                target: "audit",
                event = "host.revocation_applied",
                user_id = %user_id,
                device_key_id = %device.id,
                peer_server_id = peer_server_id.as_deref().unwrap_or("-"),
                ended_delegations = delegations,
                ended_sessions = sessions,
                "applied a device revocation from another server"
            );
        }
        let identity = self.identity.current();
        let device_id = unverified.device_id.to_string();
        let ack = sign_revocation_ack(
            &identity.signer,
            &RevocationAckFields {
                host: identity.server_id(),
                key_seq: identity.state.seq,
                principal_id: &unverified.principal_id,
                device_id: &device_id,
                revocation_payload: &revocation.payload,
                applied_at: &rim_s2s::timestamp(applied_at),
            },
        );
        Ok(SignedJson {
            payload: ack.payload,
            signature: ack.signature,
        })
    }

    async fn cleanup(&self) -> ServiceResult<RemoteWorkspaceCleanup> {
        Ok(RemoteWorkspaceCleanup {
            ended_delegations: self.repos.delegation_repo.end_lapsed(Utc::now()).await?,
            lapsed_subscriptions: 0,
        })
    }
}
