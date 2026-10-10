//! The relay side: joining local users to workspaces elsewhere, renewing their delegations,
//! reporting their presence and sending their device revocations to every host holding them.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use rim_core::key_identity::model::DeviceKeyId;
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::relay::model::{DueFanout, FanoutStatus, SubscriptionStatus};
use rim_core::relay::repository::{RelayRepository, SubscriptionRepository};
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_identity::{
    Delegation, DelegationParties, PublicKey, SignedStatement, UnverifiedDelegation,
    UnverifiedRevocation,
};
use rim_s2s::SharedIdentity;
use rim_s2s::revocation::{read_revocation_ack, verify_revocation_ack};
use rim_services::relay::dto::{RelayNotice, SignedJson};
use rim_services::relay::service::RelayService;
use rim_services::remote_workspace::dto::{
    CancelEntry, CancelRequest, JoinAnswer, JoinChallengeDto, JoinChallengeRequest, JoinDevice,
    JoinRequest, PresenceRequest, PresenceUpdate, RemoteJoinInput, RemoteJoinResultDto,
    RemoteSubscriptionDto, RemoteWorkspaceCleanup, RevocationAnswer, RevocationFanoutDto,
    RevocationRequest, SubscriptionResult, SubscriptionsAnswer, SubscriptionsRequest,
};
use rim_services::remote_workspace::service::{RevocationFanout, WorkspaceRelayService};
use rim_services::s2s::dto::{PeerLookup, S2sMethod, S2sRequest, S2sResponse};
use rim_services::s2s::service::{PeerService, S2sClient};
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::relay::identity::device_key;
use crate::relay::origin_of;
use crate::remote_workspace::presence::{OFFLINE_DEBOUNCE, PresenceTracker};
use crate::remote_workspace::{
    MAX_ACCEPTED_LIFETIME, MAX_DELEGATIONS_PER_REQUEST, next_revocation_attempt, peer_refusal,
};

/// Most delegations one `renewDelegations` call presents.
const MAX_RENEWALS: usize = 256;
/// Most fan-outs one pass sends.
const FANOUT_BATCH: i64 = 64;

#[derive(Clone)]
pub struct WorkspaceRelayRepos {
    pub subscription_repo: Arc<dyn SubscriptionRepository>,
    pub relay_repo: Arc<dyn RelayRepository>,
    pub key_identity_repo: Arc<dyn KeyIdentityRepository>,
    pub user_repo: Arc<dyn UserRepository>,
}

/// Sends presence reports; cloned into the tasks that wait out the debounce.
#[derive(Clone)]
struct Reporter {
    repos: WorkspaceRelayRepos,
    identity: SharedIdentity,
    client: Arc<dyn S2sClient>,
}

impl Reporter {
    /// Tell every host where the user holds a subscription that they are online or offline.
    async fn report(&self, user_id: UserId, online: bool) {
        let principal_id = match self.repos.user_repo.get_by_id(user_id).await {
            Ok(Some(user)) => user.principal_id,
            _ => None,
        };
        let Some(principal_id) = principal_id else {
            return;
        };
        let hosts = match self
            .repos
            .subscription_repo
            .subscribed_hosts(user_id, Utc::now())
            .await
        {
            Ok(hosts) => hosts,
            Err(e) => {
                warn!(error = %e, user_id = %user_id, "could not list hosts to report presence to");
                return;
            }
        };
        let own_id = self.identity.current().server_id().to_string();
        let Ok(body) = serde_json::to_vec(&PresenceRequest {
            updates: vec![PresenceUpdate {
                principal_id,
                online,
            }],
        }) else {
            return;
        };
        for host in hosts.into_iter().filter(|h| *h != own_id) {
            let sent = self
                .client
                .send(S2sRequest {
                    peer_server_id: host.clone(),
                    method: S2sMethod::Post,
                    path: "/presence".to_string(),
                    body: body.clone(),
                    content_type: None,
                    headers: Vec::new(),
                })
                .await;
            match sent {
                Ok(r) if r.status == 200 => {}
                Ok(r) => {
                    warn!(peer_server_id = %host, status = r.status, "a host refused a presence report")
                }
                Err(e) => {
                    warn!(peer_server_id = %host, error = %e, "could not report presence to a host")
                }
            }
        }
    }
}

pub struct DefaultWorkspaceRelay {
    repos: WorkspaceRelayRepos,
    identity: SharedIdentity,
    peers: Arc<dyn PeerService>,
    client: Arc<dyn S2sClient>,
    relay: Arc<dyn RelayService>,
    tracker: Arc<PresenceTracker>,
}

fn statement(s: &SignedJson) -> SignedStatement {
    SignedStatement {
        payload: s.payload.clone(),
        signature: s.signature.clone(),
    }
}

fn upstream(server_id: &str) -> ServiceError {
    ServiceError::UpstreamUnavailable {
        host_server_id: server_id.to_string(),
    }
}

impl DefaultWorkspaceRelay {
    pub fn new(
        repos: WorkspaceRelayRepos,
        identity: SharedIdentity,
        peers: Arc<dyn PeerService>,
        client: Arc<dyn S2sClient>,
        relay: Arc<dyn RelayService>,
    ) -> Self {
        Self {
            repos,
            identity,
            peers,
            client,
            relay,
            tracker: Arc::new(PresenceTracker::default()),
        }
    }

    fn own_id(&self) -> String {
        self.identity.current().server_id().to_string()
    }

    fn reporter(&self) -> Reporter {
        Reporter {
            repos: self.repos.clone(),
            identity: self.identity.clone(),
            client: Arc::clone(&self.client),
        }
    }

    /// The caller's principal id; `Validation` before they have a key identity.
    async fn principal(&self, user_id: UserId) -> ServiceResult<String> {
        self.repos
            .user_repo
            .get_by_id(user_id)
            .await?
            .ok_or(ServiceError::Unauthenticated)?
            .principal_id
            .ok_or_else(|| ServiceError::Validation("this account has no key identity".to_string()))
    }

    /// Check a delegation one of the caller's devices signed for this relay: it names the
    /// caller, a device of theirs that is not revoked, this relay and one of its keys, and
    /// its signature verifies. `host` is the host it must name, when known.
    async fn own_delegation(
        &self,
        user_id: UserId,
        principal_id: &str,
        delegation: &SignedJson,
        host: Option<&str>,
    ) -> ServiceResult<Delegation> {
        let unverified = UnverifiedDelegation::read(&statement(delegation))
            .map_err(crate::util::statement_refused)?;
        let fields = unverified.fields().clone();
        if fields.principal_id != principal_id {
            return Err(ServiceError::Forbidden(
                "the delegation names another principal".to_string(),
            ));
        }
        let host = host.unwrap_or(&fields.host);
        if !rim_s2s::is_server_id(host) || host == self.own_id() {
            return Err(ServiceError::Validation(
                "the delegation must name another server as its host".to_string(),
            ));
        }
        let device = self
            .repos
            .key_identity_repo
            .get_device(user_id, fields.device_id)
            .await?
            .filter(|d| d.revoked_at.is_none())
            .ok_or_else(|| {
                ServiceError::Forbidden("the delegation's device is unknown or revoked".to_string())
            })?;
        let identity = self.identity.current();
        let hashes = rim_s2s::delegable_key_hashes(&identity.log)
            .map_err(|e| ServiceError::internal(anyhow::anyhow!("own key log: {e}")))?;
        unverified
            .verify(
                &device_key(&device.public_key)?,
                &DelegationParties {
                    host,
                    relay: identity.server_id(),
                    relay_key_hashes: &hashes,
                },
                Utc::now(),
            )
            .map_err(|e| ServiceError::Validation(format!("delegation: {e}")))
    }

    /// Pin the host at `origin` under `host_server_id`.
    async fn pin_host(&self, host_server_id: &str, origin: &str) -> ServiceResult<()> {
        if !rim_s2s::is_server_id(host_server_id) {
            return Err(ServiceError::Validation(
                "hostServerId is not a server id".to_string(),
            ));
        }
        if host_server_id == self.own_id() {
            return Err(ServiceError::Validation(
                "this server hosts that workspace; join it directly".to_string(),
            ));
        }
        let origin = origin_of(origin)
            .filter(|o| o == origin.trim_end_matches('/'))
            .ok_or_else(|| {
                ServiceError::Validation("hostOrigin must be scheme://host[:port]".to_string())
            })?;
        self.peers.pin(&origin, Some(host_server_id)).await?;
        Ok(())
    }

    async fn post(&self, host: &str, path: &str, body: Vec<u8>) -> ServiceResult<S2sResponse> {
        self.client
            .send(S2sRequest {
                peer_server_id: host.to_string(),
                method: S2sMethod::Post,
                path: path.to_string(),
                body,
                content_type: None,
                headers: Vec::new(),
            })
            .await
            .map_err(|e| {
                warn!(peer_server_id = %host, error = %e, path, "a request to a host failed");
                upstream(host)
            })
    }

    /// Present delegations to one host; one result per delegation, in order.
    async fn present(&self, host: &str, delegations: &[SignedJson]) -> Vec<SubscriptionResult> {
        let refused_all = |reason: &str| {
            delegations
                .iter()
                .map(|_| SubscriptionResult::refused(None, reason))
                .collect()
        };
        let Ok(body) = serde_json::to_vec(&SubscriptionsRequest {
            delegations: delegations.to_vec(),
        }) else {
            return refused_all("internal");
        };
        let response = match self.post(host, "/subscriptions", body).await {
            Ok(response) => response,
            Err(_) => return refused_all("upstream_unavailable"),
        };
        if response.status != 200 {
            let code = response
                .error_code()
                .unwrap_or_else(|| "upstream_unavailable".to_string());
            return refused_all(&code);
        }
        match serde_json::from_slice::<SubscriptionsAnswer>(&response.body) {
            Ok(answer) if answer.subscriptions.len() == delegations.len() => answer.subscriptions,
            _ => refused_all("upstream_unavailable"),
        }
    }

    /// A host's answer to a revocation, if its acknowledgement verifies under the host's key
    /// and names this revocation.
    async fn verified_ack(
        &self,
        host: &str,
        due: &DueFanout,
        device_id: Uuid,
        response: &S2sResponse,
    ) -> Option<SignedJson> {
        let answer: RevocationAnswer = serde_json::from_slice(&response.body).ok()?;
        let signed = statement(&answer.ack);
        let read = read_revocation_ack(&signed).ok()?;
        let keys = match self.peers.resolve(host, None, Some(read.key_seq)).await {
            Ok(PeerLookup::Ready(keys)) => keys,
            _ => return None,
        };
        let key = PublicKey::parse(keys.key_for_seq(read.key_seq)?, "host key").ok()?;
        let ack = verify_revocation_ack(&key, &signed).ok()?;
        (ack.host == host
            && ack.principal_id == due.principal_id
            && ack.device_id == device_id.to_string()
            && ack.acknowledges(&due.revocation_payload))
        .then_some(answer.ack)
    }

    /// Send one revocation to one host and record what came of it. Returns whether it was
    /// acknowledged.
    async fn send_revocation(&self, due: &DueFanout) -> ServiceResult<bool> {
        let host = &due.fanout.host_server_id;
        let attempts = due.fanout.attempts.saturating_add(1);
        let revocation = SignedJson {
            payload: due.revocation_payload.clone(),
            signature: due.revocation_signature.clone(),
        };
        let device_id = UnverifiedRevocation::read(&statement(&revocation))
            .map_err(crate::util::statement_refused)?
            .device_id;
        let body = serde_json::to_vec(&RevocationRequest {
            principal_id: due.principal_id.clone(),
            revocation,
        })
        .map_err(ServiceError::internal)?;
        let response = self.post(host, "/revocations", body).await;
        let ack = match &response {
            Ok(r) if r.status == 200 => self.verified_ack(host, due, device_id, r).await,
            _ => None,
        };
        let subscription = &self.repos.subscription_repo;
        if let Some(ack) = ack {
            subscription
                .record_fanout_ack(
                    due.fanout.device_key_id,
                    host,
                    attempts,
                    &ack.payload,
                    &ack.signature,
                )
                .await?;
            info!(peer_server_id = %host, device_key_id = %due.fanout.device_key_id, attempts, "a host acknowledged a revocation");
            self.relay.publish(RelayNotice::RevocationAcked {
                user_id: due.user_id,
                device_id,
                host_server_id: host.clone(),
                ack,
            });
            return Ok(true);
        }
        let (status, next) = match (&response, next_revocation_attempt(due.revoked_at, attempts)) {
            (Ok(r), _) if r.status == 404 => (FanoutStatus::GaveUp, Utc::now()),
            (_, Some(next)) => (FanoutStatus::Pending, next),
            (_, None) => (FanoutStatus::GaveUp, Utc::now()),
        };
        if status == FanoutStatus::GaveUp {
            warn!(peer_server_id = %host, device_key_id = %due.fanout.device_key_id, attempts, "gave up sending a revocation to a host");
        }
        subscription
            .record_fanout_attempt(due.fanout.device_key_id, host, attempts, status, next)
            .await?;
        Ok(false)
    }
}

#[async_trait]
impl WorkspaceRelayService for DefaultWorkspaceRelay {
    #[instrument(name = "relay.remote_join_challenge", skip_all, fields(actor_id = %actor.user_id))]
    async fn remote_join_challenge(
        &self,
        actor: Actor,
        host_server_id: String,
        host_origin: String,
        invite_token: Option<String>,
    ) -> ServiceResult<JoinChallengeDto> {
        self.principal(actor.user_id).await?;
        self.pin_host(&host_server_id, &host_origin).await?;
        let body = serde_json::to_vec(&JoinChallengeRequest { invite_token })
            .map_err(ServiceError::internal)?;
        let response = self.post(&host_server_id, "/join/challenge", body).await?;
        if response.status != 200 {
            return Err(peer_refusal(&host_server_id, &response));
        }
        serde_json::from_slice(&response.body).map_err(|_| upstream(&host_server_id))
    }

    #[instrument(name = "relay.join_remote_workspace", skip_all, fields(actor_id = %actor.user_id))]
    async fn join_remote_workspace(
        &self,
        actor: Actor,
        input: RemoteJoinInput,
    ) -> ServiceResult<RemoteJoinResultDto> {
        let principal_id = self.principal(actor.user_id).await?;
        let delegation = self
            .own_delegation(
                actor.user_id,
                &principal_id,
                &input.delegation,
                Some(&input.host_server_id),
            )
            .await?;
        let document = self
            .relay
            .local_identity(principal_id.clone())
            .await?
            .ok_or_else(|| {
                ServiceError::Validation("this account has no key identity".to_string())
            })?;
        self.pin_host(&input.host_server_id, &input.host_origin)
            .await?;
        let body = serde_json::to_vec(&JoinRequest {
            invite_token: input.invite_token.clone(),
            key_log: document.key_log,
            devices: document
                .devices
                .into_iter()
                .map(|d| JoinDevice {
                    certificate: d.certificate,
                    revocation: d.revocation,
                })
                .collect(),
            card: document.card,
            proof: input.proof,
            delegation: input.delegation,
        })
        .map_err(ServiceError::internal)?;
        let host = input.host_server_id;
        let response = self.post(&host, "/join", body).await?;
        if response.status != 200 {
            return Err(peer_refusal(&host, &response));
        }
        let answer: JoinAnswer =
            serde_json::from_slice(&response.body).map_err(|_| upstream(&host))?;
        if answer.workspace_id != delegation.workspace_id {
            return Err(upstream(&host));
        }
        let now = Utc::now();
        self.repos
            .subscription_repo
            .upsert_subscription(
                actor.user_id,
                &host,
                answer.workspace_id,
                SubscriptionStatus::Active,
                Some(delegation.expires_at.min(now + MAX_ACCEPTED_LIFETIME)),
            )
            .await?;
        // The host now holds the person: give it the whole identity, devices' encryption
        // readiness included.
        let relay = Arc::clone(&self.relay);
        let user_id = actor.user_id;
        tokio::spawn(async move {
            if let Err(e) = relay.identity_changed(user_id).await {
                warn!(error = %e, user_id = %user_id, "could not push an identity to a new host");
            }
        });
        info!(peer_server_id = %host, workspace_id = %answer.workspace_id, "joined a workspace on another host");
        Ok(RemoteJoinResultDto {
            host_server_id: host,
            workspace_id: answer.workspace_id,
            user_id: answer.user_id,
            role: answer.role,
        })
    }

    #[instrument(name = "relay.renew_delegations", skip_all, fields(actor_id = %actor.user_id, delegations = delegations.len()))]
    async fn renew_delegations(
        &self,
        actor: Actor,
        delegations: Vec<SignedJson>,
    ) -> ServiceResult<Vec<RemoteSubscriptionDto>> {
        if delegations.is_empty() || delegations.len() > MAX_RENEWALS {
            return Err(ServiceError::Validation(format!(
                "present 1 to {MAX_RENEWALS} delegations"
            )));
        }
        let principal_id = self.principal(actor.user_id).await?;
        let mut checked = Vec::with_capacity(delegations.len());
        for delegation in &delegations {
            checked.push(
                self.own_delegation(actor.user_id, &principal_id, delegation, None)
                    .await?,
            );
        }
        let mut by_host: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, delegation) in checked.iter().enumerate() {
            by_host
                .entry(delegation.host.clone())
                .or_default()
                .push(index);
        }
        let mut results: Vec<Option<RemoteSubscriptionDto>> = vec![None; delegations.len()];
        for (host, indices) in by_host {
            for chunk in indices.chunks(MAX_DELEGATIONS_PER_REQUEST) {
                let presented: Vec<SignedJson> =
                    chunk.iter().map(|i| delegations[*i].clone()).collect();
                let answers = self.present(&host, &presented).await;
                for (index, answer) in chunk.iter().zip(answers) {
                    let workspace_id = checked[*index].workspace_id;
                    let expires_at = answer
                        .expires_at
                        .as_deref()
                        .and_then(|t| t.parse::<chrono::DateTime<Utc>>().ok());
                    let status = if answer.is_active() {
                        SubscriptionStatus::Active
                    } else {
                        SubscriptionStatus::Ended
                    };
                    if answer.is_active()
                        || answer.reason.as_deref() != Some("upstream_unavailable")
                    {
                        self.repos
                            .subscription_repo
                            .upsert_subscription(
                                actor.user_id,
                                &host,
                                workspace_id,
                                status,
                                expires_at,
                            )
                            .await?;
                    }
                    results[*index] = Some(RemoteSubscriptionDto {
                        host_server_id: host.clone(),
                        workspace_id,
                        status: answer.status,
                        expires_at,
                        reason: answer.reason,
                    });
                }
            }
        }
        Ok(results.into_iter().flatten().collect())
    }

    async fn remote_subscriptions(
        &self,
        actor: Actor,
    ) -> ServiceResult<Vec<RemoteSubscriptionDto>> {
        Ok(self
            .repos
            .subscription_repo
            .list_subscriptions(actor.user_id)
            .await?
            .into_iter()
            .map(|s| RemoteSubscriptionDto {
                host_server_id: s.host_server_id,
                workspace_id: s.workspace_id,
                status: s.status.as_str().to_string(),
                expires_at: s.expires_at,
                reason: None,
            })
            .collect())
    }

    async fn device_revocation_status(
        &self,
        actor: Actor,
        device_id: Uuid,
    ) -> ServiceResult<Vec<RevocationFanoutDto>> {
        let device = self
            .repos
            .key_identity_repo
            .get_device(actor.user_id, device_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        Ok(self
            .repos
            .subscription_repo
            .list_fanouts(device.id)
            .await?
            .into_iter()
            .map(|f| RevocationFanoutDto {
                host_server_id: f.host_server_id,
                status: f.status,
                ack: f
                    .ack_payload
                    .zip(f.ack_signature)
                    .map(|(payload, signature)| SignedJson { payload, signature }),
                attempts: f.attempts,
            })
            .collect())
    }

    #[instrument(name = "relay.leave_relay_subscriptions", skip_all, fields(actor_id = %actor.user_id))]
    async fn leave_relay(&self, actor: Actor) -> ServiceResult<bool> {
        let principal_id = self
            .repos
            .user_repo
            .get_by_id(actor.user_id)
            .await?
            .and_then(|u| u.principal_id);
        if let Some(principal_id) = principal_id {
            let mut by_host: BTreeMap<String, Vec<CancelEntry>> = BTreeMap::new();
            for subscription in self
                .repos
                .subscription_repo
                .list_subscriptions(actor.user_id)
                .await?
                .into_iter()
                .filter(|s| s.status != SubscriptionStatus::Ended)
            {
                by_host
                    .entry(subscription.host_server_id)
                    .or_default()
                    .push(CancelEntry {
                        workspace_id: subscription.workspace_id,
                        principal_id: principal_id.clone(),
                    });
            }
            for (host, entries) in by_host {
                for chunk in entries.chunks(MAX_DELEGATIONS_PER_REQUEST) {
                    let body = serde_json::to_vec(&CancelRequest {
                        subscriptions: chunk.to_vec(),
                    })
                    .map_err(ServiceError::internal)?;
                    match self.post(&host, "/subscriptions/cancel", body).await {
                        Ok(r) if r.status == 200 => {}
                        Ok(r) => {
                            warn!(peer_server_id = %host, status = r.status, "a host refused to cancel subscriptions")
                        }
                        Err(_) => {}
                    }
                }
            }
            self.repos
                .subscription_repo
                .end_subscriptions(actor.user_id)
                .await?;
        }
        self.relay.leave_relay(actor).await
    }

    fn stream_opened(&self, user_id: UserId) {
        if self.tracker.opened(user_id) {
            let reporter = self.reporter();
            tokio::spawn(async move { reporter.report(user_id, true).await });
        }
    }

    fn stream_closed(&self, user_id: UserId) {
        let Some(generation) = self.tracker.closed(user_id) else {
            return;
        };
        let reporter = self.reporter();
        let tracker = Arc::clone(&self.tracker);
        tokio::spawn(async move {
            tokio::time::sleep(OFFLINE_DEBOUNCE).await;
            if tracker.settle(user_id, generation) {
                reporter.report(user_id, false).await;
            }
        });
    }

    async fn send_due_revocations(&self) -> ServiceResult<u64> {
        let due = self
            .repos
            .subscription_repo
            .due_fanouts(Utc::now(), FANOUT_BATCH)
            .await?;
        let attempts = due.len() as u64;
        for fanout in &due {
            if let Err(e) = self.send_revocation(fanout).await {
                warn!(error = %e, peer_server_id = %fanout.fanout.host_server_id, "could not send a revocation");
            }
        }
        Ok(attempts)
    }

    async fn cleanup(&self) -> ServiceResult<RemoteWorkspaceCleanup> {
        Ok(RemoteWorkspaceCleanup {
            ended_delegations: 0,
            lapsed_subscriptions: self
                .repos
                .subscription_repo
                .lapse_expired(Utc::now())
                .await?,
        })
    }
}

#[async_trait]
impl RevocationFanout for DefaultWorkspaceRelay {
    async fn revocation_committed(
        &self,
        user_id: UserId,
        device_key_id: DeviceKeyId,
    ) -> ServiceResult<()> {
        let own_id = self.own_id();
        let hosts: Vec<String> = self
            .repos
            .relay_repo
            .hosts_of_user(user_id)
            .await?
            .into_iter()
            .filter(|h| *h != own_id)
            .collect();
        if hosts.is_empty() {
            return Ok(());
        }
        let added = self
            .repos
            .subscription_repo
            .insert_fanouts(device_key_id, &hosts, Utc::now())
            .await?;
        info!(user_id = %user_id, device_key_id = %device_key_id, hosts = added, "sending a device revocation to the hosts holding its person");
        self.send_due_revocations().await?;
        Ok(())
    }
}
