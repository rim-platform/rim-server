//! This server as its users' relay: contacts, blocks and the request inbox, the queue of host
//! events with per-device cursors, identities served to and fetched from other servers, and
//! what hosts deliver.

pub mod abuse;
pub(crate) mod identity;
pub(crate) mod inbox;
pub(crate) mod policy;

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use sea_orm::DatabaseConnection;
use tokio::sync::broadcast;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use rim_core::dm_policy::repository::DmPolicyRepository;
use rim_core::e2ee::repository::E2eeRepository;
use rim_core::host::model::UserKind;
use rim_core::host::repository::HostRepository;
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::relay::model::{
    DmRequest, DmRequestId, DmRequestStatus, NewAbuseReport, NewDmRequest, NewRelayEnvelope,
    NewRelayItem, RelayItemRecord,
};
use rim_core::relay::repository::{RelayAbuseRepository, RelayRepository, SubscriptionRepository};
use rim_core::repository::error::RepoErrorKind;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_identity::{ProfileCard, SignedStatement, UnverifiedDeviceRequest, is_principal_id};
use rim_infra_db::{TransactionScope, begin_transaction};
use rim_s2s::SharedIdentity;
use rim_s2s::host::{read_host_event, verify_host_event};
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::dm_policy::dto::DmOperatorPolicy;
use rim_services::e2ee::command::MlsCaller;
use rim_services::e2ee::{ACCEPTED_CIPHER_SUITES, PREFERRED_CIPHER_SUITE};
use rim_services::relay::dto::{
    BlockedPrincipalDto, ContactDto, DeliverItem, DeliverResponse, DmRequestDto, IdentityDocument,
    OutboxPage, PeerClaimRequest, PeerClaimResponse, PeerKeyPackage, QueueSpace, RefusedItem,
    RelayCleanup, RelayItemDto, RelayItemPageDto, RelayNotice, RemoteIdentityDto, RemoteServerDto,
    SignedJson,
};
use rim_services::relay::service::RelayService;
use rim_services::s2s::dto::{PeerKeysDto, PeerLookup, S2sMethod, S2sRequest};
use rim_services::s2s::service::{PeerService, S2sClient};
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::quota::{DailyCounter, Quotas};
use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::relay::identity::{local_document, verify_identity};
use crate::relay::inbox::{Filing, InboxFacts, MAX_REQUESTS_PER_HOST_PER_DAY, SENDER_WINDOW, file};
use crate::relay::policy::{DmPolicy, PolicySource};
use crate::repos::AppRepos;

/// Most items one delivery carries.
pub(crate) const MAX_DELIVER_ITEMS: usize = 256;
/// Most bytes one delivery or outbox page carries.
pub(crate) const MAX_DELIVER_BYTES: i64 = 8 * 1024 * 1024;
/// Most bytes of items queued from one peer per day, across all recipients, by default.
pub const QUEUED_BYTES_PER_PEER_PER_DAY: u64 = 2 * 1024 * 1024 * 1024;
/// Most relay URLs a contact carries.
const MAX_CONTACT_RELAYS: usize = 8;
/// Most contacts one user holds.
pub(crate) const MAX_CONTACTS_PER_USER: i64 = 1000;
/// Longest relay URL, in bytes.
const MAX_RELAY_URL_LEN: usize = 256;
/// Longest report reason, in characters.
const MAX_REPORT_REASON_CHARS: usize = 1000;
/// How long a fetched identity is served from the cache.
const IDENTITY_CACHE_TTL: Duration = Duration::hours(24);
/// How long an ignored request is kept, so its conversation stays dropped.
const IGNORED_REQUEST_TTL: Duration = Duration::days(30);
/// Most devices one claim names.
const MAX_CLAIM_DEVICES: usize = 64;
/// Claims per stranger principal per day.
const STRANGER_CLAIMS_PER_DAY: u32 = 20;
/// Largest card a DM invite may carry.
const MAX_CARD_BYTES: usize = 128 * 1024;
/// How long an accepted conversation's row lives after the last item delivered for it: over a
/// year, so a conversation whose members write once a year stays open, while one that went
/// silent, or whose host is gone, is let go.
const ACCEPTED_REQUEST_TTL: Duration = Duration::days(400);
/// How far an accepted row's expiry may fall behind before an item moves it out again, so the
/// row is not written for every item.
const ACCEPTED_REFRESH_AFTER: Duration = Duration::days(1);
/// When an accepted conversation's row expires if nothing is delivered for it from `now` on.
fn accepted_expiry(now: DateTime<Utc>) -> DateTime<Utc> {
    now + ACCEPTED_REQUEST_TTL
}

/// Why a pending `request` could not be decided: `NotFound` once it has expired, as for an
/// unknown request, else `Conflict` because it was decided already.
fn undecided(request: &DmRequest) -> ServiceError {
    if request.expires_at <= Utc::now() {
        ServiceError::NotFound
    } else {
        ServiceError::Conflict("the request was decided already".to_string())
    }
}

#[derive(Clone)]
pub struct RelayRepos {
    pub relay_repo: Arc<dyn RelayRepository>,
    pub host_repo: Arc<dyn HostRepository>,
    pub key_identity_repo: Arc<dyn KeyIdentityRepository>,
    pub e2ee_repo: Arc<dyn E2eeRepository>,
    pub user_repo: Arc<dyn UserRepository>,
    pub subscription_repo: Arc<dyn SubscriptionRepository>,
    /// Principals this server refuses, and abuse reports.
    pub relay_abuse_repo: Arc<dyn RelayAbuseRepository>,
    /// Workspaces' DM rules and each user's choices.
    pub dm_policy_repo: Arc<dyn DmPolicyRepository>,
}

/// Limits of the relay's queues and its users' traffic.
#[derive(Debug, Clone, Copy)]
pub struct RelaySettings {
    /// Bytes queued per local user before the oldest items are dropped.
    pub queue_bytes_per_user: i64,
    /// How long items and requests are kept.
    pub retention: Duration,
    /// Forwarded requests and DM requests per user.
    pub quotas: Quotas,
    /// Bytes of items queued from one peer per day, across all recipients; past it the peer's
    /// items are refused (`limited`). Items of a workspace the recipient subscribed to are not
    /// counted.
    pub queued_bytes_per_peer_per_day: u64,
}

pub struct DefaultRelayService {
    repos: RelayRepos,
    db: Option<DatabaseConnection>,
    identity: SharedIdentity,
    peers: Arc<dyn PeerService>,
    client: Arc<dyn S2sClient>,
    settings: RelaySettings,
    notices: broadcast::Sender<RelayNotice>,
    stranger_claims: RegistrationRateLimiter,
    /// Requests each device sends through `/api/remote/…` per minute.
    forwards_per_device: RegistrationRateLimiter,
    /// Requests each user sends through `/api/remote/…` per day.
    forwards_per_user: DailyCounter,
    /// Bytes of items queued from each peer per day, by server id.
    queued_per_peer: DailyCounter,
    /// The operator's DM settings.
    dm_policy: DmOperatorPolicy,
}

/// Validate a principal id given by a client.
fn check_principal(principal_id: &str) -> ServiceResult<()> {
    if is_principal_id(principal_id) {
        Ok(())
    } else {
        Err(ServiceError::Validation(
            "principalId is not a principal id".to_string(),
        ))
    }
}

/// Relay URLs as on a contact link: 1 to 8, each 1 to 256 bytes of printable ASCII without
/// `?`, `#` or `@`.
pub(crate) fn check_relay_urls(urls: &[String]) -> ServiceResult<()> {
    if urls.is_empty() || urls.len() > MAX_CONTACT_RELAYS {
        return Err(ServiceError::Validation(format!(
            "give 1 to {MAX_CONTACT_RELAYS} relay URLs"
        )));
    }
    for url in urls {
        let printable = url.bytes().all(|b| (0x21..=0x7e).contains(&b));
        if url.is_empty()
            || url.len() > MAX_RELAY_URL_LEN
            || !printable
            || url.contains(['?', '#', '@'])
        {
            return Err(ServiceError::Validation(format!(
                "a relay URL is 1 to {MAX_RELAY_URL_LEN} bytes of printable ASCII without ?, # or @"
            )));
        }
    }
    Ok(())
}

/// The origin (`scheme://host[:port]`) of a relay URL.
pub(crate) fn origin_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    match parsed.scheme() {
        "https" | "http" => {}
        _ => return None,
    }
    Some(parsed.origin().ascii_serialization())
}

/// The relays a card payload lists, verified when it was stored.
pub(crate) fn card_relays(card_payload: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(card_payload)
        .ok()
        .and_then(|v| v.get("relays").cloned())
        .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
        .unwrap_or_default()
}

/// Whether a card's relays include one at `origin`.
pub(crate) fn relays_include_origin(relays: &[String], origin: &str) -> bool {
    relays
        .iter()
        .filter_map(|url| origin_of(url))
        .any(|o| o == origin)
}

/// The relays a card lists as a contact's relay URLs: its first ones when they follow the
/// contact link rules, else none.
pub(crate) fn contact_relays(card_payload: &str) -> Vec<String> {
    let relays: Vec<String> = card_relays(card_payload)
        .into_iter()
        .take(MAX_CONTACT_RELAYS)
        .collect();
    if check_relay_urls(&relays).is_ok() {
        relays
    } else {
        Vec::new()
    }
}

fn item_dto(record: RelayItemRecord) -> RelayItemDto {
    RelayItemDto {
        id: record.id,
        host_server_id: record.host_server_id,
        workspace_id: record.workspace_id,
        envelope: SignedJson {
            payload: record.payload,
            signature: record.signature,
        },
    }
}

fn upstream(server_id: &str) -> ServiceError {
    ServiceError::upstream(server_id)
}

/// What a delivered envelope says about where it belongs.
struct ReadEnvelope {
    workspace_id: Uuid,
    stream: String,
    seq: Option<i64>,
    conversation_id: Option<Uuid>,
    invite: Option<Invite>,
    local: bool,
    /// The delivering host's pinned origin; `None` for this server's own events.
    host_origin: Option<String>,
    /// The event's `user_id`: the id its host gives the user the event is about.
    user_id: Option<Uuid>,
    /// The event removes `user_id` from its conversation (`MemberRemovedFromConversation`).
    removes_member: bool,
    /// The event decides a join request (`JoinRequestDecided`).
    join_decided: bool,
    /// The event says a member left the workspace (`MemberLeft`).
    departed: bool,
}

/// The request inbox's decision for one recipient of an item.
enum Filed {
    /// Queue the item, held by the request when one is given.
    Queue(Option<DmRequestId>),
    /// Do not queue it; a refusal reason when the host is told.
    Done(Option<&'static str>),
}

/// What the request inbox makes of the sender an invite from another host names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InviteSender {
    /// The card verified against the sender's key log, and the host speaks for them.
    Verified,
    /// The sender's identity could not be obtained, here or from the host: the invite is taken
    /// as from a stranger.
    Unverified,
    /// Refused with a reason the host sees.
    Refused(&'static str),
}

/// The parts of a `DmInvite` the request inbox keeps.
struct Invite {
    from_principal_id: String,
    card: Option<SignedJson>,
}

impl DefaultRelayService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repos: RelayRepos,
        db: Option<DatabaseConnection>,
        identity: SharedIdentity,
        peers: Arc<dyn PeerService>,
        client: Arc<dyn S2sClient>,
        settings: RelaySettings,
    ) -> Self {
        let (notices, _) = broadcast::channel(1024);
        Self {
            repos,
            db,
            identity,
            peers,
            client,
            settings,
            notices,
            stranger_claims: RegistrationRateLimiter::new(24 * 60 * 60, STRANGER_CLAIMS_PER_DAY),
            forwards_per_device: RegistrationRateLimiter::new(
                60,
                settings.quotas.forwards_per_device_per_min,
            ),
            forwards_per_user: DailyCounter::per_day(u64::from(
                settings.quotas.forwards_per_user_per_day,
            )),
            queued_per_peer: DailyCounter::per_day(settings.queued_bytes_per_peer_per_day),
            dm_policy: DmOperatorPolicy::default(),
        }
    }

    /// Apply the operator's DM settings.
    pub fn with_dm_policy(mut self, dm_policy: DmOperatorPolicy) -> Self {
        self.dm_policy = dm_policy;
        self
    }

    /// Whether `peer_server_id` may read a local user's identity and claim their key packages:
    /// the operator's inbound setting admits it, or the user holds an active subscription to a
    /// workspace it hosts.
    async fn admits_peer(&self, user_id: UserId, peer_server_id: &str) -> ServiceResult<bool> {
        if self.dm_policy.admits_inbound(peer_server_id) {
            return Ok(true);
        }
        self.subscribed_at(user_id, peer_server_id).await
    }

    /// Whether a local user holds an active subscription to a workspace `host_server_id` hosts.
    async fn subscribed_at(&self, user_id: UserId, host_server_id: &str) -> ServiceResult<bool> {
        Ok(self
            .repos
            .subscription_repo
            .subscribed_hosts(user_id, Utc::now())
            .await?
            .iter()
            .any(|host| host == host_server_id))
    }

    /// The server a DM invite's sender writes from: the host that delivered it, or for an own
    /// event the home relay of a creator from another server, else this one.
    async fn sender_server(
        &self,
        repos: &RelayRepos,
        peer_server_id: &str,
        read: &ReadEnvelope,
        from_principal_id: &str,
    ) -> ServiceResult<String> {
        if !read.local {
            return Ok(peer_server_id.to_string());
        }
        match repos.host_repo.find_user(from_principal_id).await? {
            Some((user_id, UserKind::Remote)) => Ok(repos
                .host_repo
                .home_relay(user_id)
                .await?
                .unwrap_or_default()),
            _ => Ok(peer_server_id.to_string()),
        }
    }

    async fn begin(&self) -> ServiceResult<(Option<TransactionScope>, RelayRepos)> {
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_relay(),
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

    fn notify(&self, notice: RelayNotice) {
        let _ = self.notices.send(notice);
    }

    fn own_id(&self) -> String {
        self.identity.current().server_id().to_string()
    }

    /// The session's device: the caller's and not revoked.
    async fn session_device(&self, caller: &MlsCaller) -> ServiceResult<DeviceKeyId> {
        crate::e2ee::session_device(self.repos.e2ee_repo.as_ref(), caller)
            .await
            .map(|d| d.device_key_id)
    }

    async fn request_dto(&self, request: DmRequest) -> ServiceResult<DmRequestDto> {
        let held_items = self
            .repos
            .relay_repo
            .held_items(request.id)
            .await?
            .into_iter()
            .map(item_dto)
            .collect();
        Ok(DmRequestDto {
            id: request.id,
            from_principal_id: request.from_principal_id,
            card: SignedJson {
                payload: request.card_payload,
                signature: request.card_signature,
            },
            host_server_id: request.host_server_id,
            conversation_id: request.conversation_id,
            held_items,
            status: request.status,
            created_at: request.created_at,
            expires_at: request.expires_at,
        })
    }

    /// The actor's request `id`, or `NotFound` when there is none or it has expired.
    async fn own_request(&self, actor: Actor, id: DmRequestId) -> ServiceResult<DmRequest> {
        self.repos
            .relay_repo
            .get_request(actor.user_id, id)
            .await?
            .filter(|request| request.expires_at > Utc::now())
            .ok_or(ServiceError::NotFound)
    }

    /// A pinned peer's keys, ready to verify what it signed.
    async fn peer_keys(&self, server_id: &str, key_seq: Option<u32>) -> ServiceResult<PeerKeysDto> {
        match self.peers.resolve(server_id, None, key_seq).await? {
            PeerLookup::Ready(keys) => Ok(keys),
            PeerLookup::Unknown => Err(ServiceError::Forbidden("unknown peer".to_string())),
            PeerLookup::Forked => Err(ServiceError::Forbidden("peer forked".to_string())),
            PeerLookup::Blocked => Err(ServiceError::Forbidden("peer blocked".to_string())),
        }
    }

    /// Check an envelope a host delivered: it names that host, verifies under its key at the
    /// envelope's `keySeq`, and has a space id. `None` when it does not.
    async fn read_envelope(
        &self,
        peer_server_id: &str,
        keys: &PeerKeysDto,
        envelope: &SignedJson,
    ) -> Option<ReadEnvelope> {
        let statement = SignedStatement {
            payload: envelope.payload.clone(),
            signature: envelope.signature.clone(),
        };
        let unverified = read_host_event(&statement).ok()?;
        if unverified.host != peer_server_id {
            return None;
        }
        let key_text = match keys.key_for_seq(unverified.key_seq) {
            Some(key) => key.to_string(),
            None if unverified.key_seq > keys.head_seq => self
                .peer_keys(peer_server_id, Some(unverified.key_seq))
                .await
                .ok()?
                .key_for_seq(unverified.key_seq)?
                .to_string(),
            None => return None,
        };
        let key = rim_identity::PublicKey::parse(&key_text, "peer key").ok()?;
        let event = verify_host_event(&key, &statement).ok()?;
        let mut read = envelope_fields(event, false)?;
        read.host_origin = Some(keys.origin.clone());
        Some(read)
    }
}

/// The card of a verified identity document, when it verifies against the log's head.
fn verified_card(document: &IdentityDocument) -> Option<ProfileCard> {
    verify_identity(document).ok()?.card.map(|(card, _)| card)
}

/// `card` verified as `principal_id`'s card against `key_log`, their key log: `None` when the
/// log or the card does not verify, or the card was signed before the log's head.
fn card_of(principal_id: &str, key_log: Vec<SignedJson>, card: &SignedJson) -> Option<ProfileCard> {
    verified_card(&IdentityDocument {
        principal_id: principal_id.to_string(),
        key_log,
        devices: Vec::new(),
        card: Some(card.clone()),
        username: None,
    })
}

/// Where a host event belongs. `local` marks an event of this server's own direct space, filed
/// for a local member: only the request inbox keeps it, the live path carries the rest.
fn envelope_fields(event: rim_s2s::host::HostEvent, local: bool) -> Option<ReadEnvelope> {
    {
        let workspace_id = Uuid::parse_str(&event.workspace_id).ok()?;
        let seq = match event.seq {
            Some(seq) => Some(i64::try_from(seq).ok()?),
            None => None,
        };
        let conversation_id = event
            .event
            .get("conversation_id")
            .and_then(|v| v.as_str())
            .and_then(|v| Uuid::parse_str(v).ok());
        let user_id = event
            .event
            .get("user_id")
            .and_then(|v| v.as_str())
            .and_then(|v| Uuid::parse_str(v).ok());
        let event_type = event.event.get("type").and_then(|v| v.as_str());
        let removes_member = event_type == Some("MemberRemovedFromConversation");
        let join_decided = event_type == Some("JoinRequestDecided");
        let departed = event_type == Some("MemberLeft");
        let invite = (event_type == Some("DmInvite")).then(|| Invite {
            from_principal_id: event
                .event
                .get("from_principal_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            card: event
                .event
                .get("from_card")
                .and_then(|v| serde_json::from_value::<SignedJson>(v.clone()).ok())
                .filter(|c| c.payload.len() <= MAX_CARD_BYTES),
        });
        Some(ReadEnvelope {
            workspace_id,
            stream: event.stream,
            seq,
            conversation_id,
            invite,
            local,
            host_origin: None,
            user_id,
            removes_member,
            join_decided,
            departed,
        })
    }
}

impl DefaultRelayService {
    /// File one recipient's copy of an envelope. Returns a refusal reason, if any.
    #[allow(clippy::too_many_arguments)]
    async fn file_for(
        &self,
        repos: &RelayRepos,
        peer_server_id: &str,
        read: &ReadEnvelope,
        envelope: &SignedJson,
        envelope_id: &mut Option<i64>,
        principal_id: &str,
        device_id: Option<Uuid>,
        notices: &mut Vec<RelayNotice>,
    ) -> ServiceResult<Option<&'static str>> {
        let Some((user_id, UserKind::Local)) = repos.host_repo.find_user(principal_id).await?
        else {
            return Ok(Some("unknown_principal"));
        };
        // A principal blocked here gets nothing, as if unknown.
        if repos
            .relay_abuse_repo
            .is_principal_blocked(principal_id)
            .await?
        {
            return Ok(Some("unknown_principal"));
        }
        let device_key_id = match device_id {
            Some(device_id) => {
                match repos
                    .key_identity_repo
                    .get_device(user_id, device_id)
                    .await?
                {
                    Some(device) if device.revoked_at.is_none() => Some(device.id),
                    _ => return Ok(Some("unknown_device")),
                }
            }
            None => None,
        };
        let now = Utc::now();
        let size =
            i32::try_from(envelope.payload.len() + envelope.signature.len()).unwrap_or(i32::MAX);
        // A workspace the user subscribed to through a delegation is not a DM: its items are
        // queued as they come.
        let subscribed = !read.local
            && repos
                .subscription_repo
                .is_subscribed(user_id, peer_server_id, read.workspace_id, now)
                .await?;
        let request_id = if subscribed {
            None
        } else {
            match self
                .inbox_filing(
                    repos,
                    peer_server_id,
                    read,
                    (user_id, principal_id),
                    size,
                    now,
                    notices,
                )
                .await?
            {
                Filed::Queue(request_id) => request_id,
                Filed::Done(refusal) => return Ok(refusal),
            }
        };

        let envelope_id = match envelope_id {
            Some(id) => *id,
            None => {
                let id = repos
                    .relay_repo
                    .insert_envelope(NewRelayEnvelope {
                        host_server_id: peer_server_id.to_string(),
                        workspace_id: read.workspace_id,
                        stream: read.stream.clone(),
                        seq: read.seq,
                        payload: envelope.payload.clone(),
                        signature: envelope.signature.clone(),
                    })
                    .await?;
                *envelope_id = Some(id);
                id
            }
        };
        repos
            .relay_repo
            .insert_item(NewRelayItem {
                user_id,
                envelope_id,
                device_key_id,
                request_id,
                size_bytes: size,
                expires_at: now + self.settings.retention,
            })
            .await?;
        if !subscribed && !read.local {
            self.queued_per_peer
                .add(peer_server_id, now, u64::try_from(size).unwrap_or(0));
        }
        // A departure on the user's own stream ends their subscription there: the host ended
        // their delegations with it.
        if subscribed && read.departed && read.stream.strip_prefix("u:") == Some(principal_id) {
            repos
                .subscription_repo
                .end_subscription(user_id, peer_server_id, read.workspace_id)
                .await?;
        }
        // A delivered decision closes the user's request at that host.
        if read.join_decided && !read.local {
            repos
                .subscription_repo
                .delete_join_request(user_id, peer_server_id, read.workspace_id)
                .await?;
        }
        notices.push(RelayNotice::Items { user_id });
        Ok(None)
    }

    /// Where the request inbox files an item of `size` bytes for one recipient, given by user
    /// and principal id: the queue, possibly held by a request, or nowhere, with any refusal.
    #[allow(clippy::too_many_arguments)]
    async fn inbox_filing(
        &self,
        repos: &RelayRepos,
        peer_server_id: &str,
        read: &ReadEnvelope,
        (user_id, principal_id): (UserId, &str),
        size: i32,
        now: DateTime<Utc>,
        notices: &mut Vec<RelayNotice>,
    ) -> ServiceResult<Filed> {
        // Items without a conversation reach a direct space's members on `e` and on their own
        // `u:` stream only.
        let direct_stream = read.conversation_id.is_none()
            && (read.stream == "e" || read.stream.strip_prefix("u:") == Some(principal_id));
        let own_stream = read.stream.strip_prefix("u:") == Some(principal_id);
        let mut facts = InboxFacts {
            is_invite: read.invite.is_some(),
            conversation_scoped: read.conversation_id.is_some(),
            direct_stream,
            join_decided: read.join_decided
                && own_stream
                && !read.local
                && read.conversation_id.is_none()
                && repos
                    .subscription_repo
                    .has_join_request(user_id, peer_server_id, read.workspace_id, now)
                    .await?,
            accepted_at_host: direct_stream
                && !read.local
                && repos
                    .relay_repo
                    .accepted_at_host(user_id, peer_server_id)
                    .await?,
            peer_bytes_left: if read.local {
                i64::MAX
            } else {
                i64::try_from(self.queued_per_peer.remaining(peer_server_id, now))
                    .unwrap_or(i64::MAX)
            },
            blocked: false,
            policy: DmPolicy::Request,
            request: None,
            pending_for_user: 0,
            filed_by_sender_today: 0,
            sender_daily_limit: i64::from(self.settings.quotas.dm_requests_per_day),
            item_bytes: i64::from(size),
            request_held_bytes: 0,
            user_held_bytes: 0,
            filed_by_host_today: 0,
            accepted_from_host: 0,
        };
        let mut found = None;
        if let Some(conversation_id) = read.conversation_id
            && let Some(request) = repos
                .relay_repo
                .find_request(user_id, peer_server_id, conversation_id)
                .await?
        {
            let held = if request.status == DmRequestStatus::Pending {
                facts.request_held_bytes = repos.relay_repo.request_held_bytes(request.id).await?;
                facts.user_held_bytes = repos.relay_repo.user_held_bytes(user_id).await?;
                repos.relay_repo.count_held(request.id).await?
            } else {
                0
            };
            facts.request = Some((request.id, request.status, held));
            found = Some(request);
        }
        if let Some(invite) = &read.invite {
            let from = invite.from_principal_id.as_str();
            facts.blocked = repos.relay_repo.is_blocked(user_id, from).await?
                || repos.relay_abuse_repo.is_principal_blocked(from).await?;
            if facts.request.is_none() {
                let mut verified = true;
                if let Some(host_origin) = &read.host_origin
                    && !facts.blocked
                {
                    facts.filed_by_host_today = repos
                        .relay_repo
                        .count_requests_via_host(peer_server_id, now - Duration::days(1))
                        .await?;
                    match self
                        .invite_sender(
                            repos,
                            peer_server_id,
                            host_origin,
                            invite,
                            facts.filed_by_host_today,
                        )
                        .await?
                    {
                        InviteSender::Verified => {}
                        InviteSender::Unverified => {
                            info!(peer_server_id = %peer_server_id, "an invite's sender could not be verified; taken as a stranger");
                            verified = false;
                        }
                        InviteSender::Refused(reason) => {
                            if reason == "limited" {
                                crate::metrics::rate_limited("dm_requests");
                            } else {
                                warn!(peer_server_id = %peer_server_id, "refused an invite whose sender the host does not speak for");
                            }
                            return Ok(Filed::Done(Some(reason)));
                        }
                    }
                }
                let own_id = self.own_id();
                let sender_server = self
                    .sender_server(repos, peer_server_id, read, from)
                    .await?;
                let source = PolicySource {
                    relay_repo: repos.relay_repo.as_ref(),
                    dm_policy_repo: repos.dm_policy_repo.as_ref(),
                    operator: &self.dm_policy,
                    own_server_id: &own_id,
                };
                facts.policy = if verified {
                    source.inbound(user_id, from, &sender_server).await?
                } else {
                    match source.stranger(user_id, &sender_server).await? {
                        DmPolicy::Deliver => DmPolicy::Request,
                        policy => policy,
                    }
                };
                // The cap bounds what other hosts store here; this server's own conversations
                // are bounded where they are created.
                if facts.policy == DmPolicy::Deliver && !read.local {
                    facts.accepted_from_host = repos
                        .relay_repo
                        .count_accepted_requests(user_id, peer_server_id)
                        .await?;
                }
                facts.pending_for_user = repos.relay_repo.count_pending_requests(user_id).await?;
                facts.user_held_bytes = repos.relay_repo.user_held_bytes(user_id).await?;
                facts.filed_by_sender_today = repos
                    .relay_repo
                    .count_requests_from(from, now - SENDER_WINDOW)
                    .await?;
            }
        }

        let filing = file(&facts);
        if let Some(request) = &found {
            Self::note_activity(
                repos,
                request,
                read,
                principal_id,
                filing == Filing::Deliver,
                now,
            )
            .await?;
        }
        let request_id = match filing {
            Filing::PeerLimited => {
                crate::metrics::rate_limited("relay_peer_bytes");
                return Ok(Filed::Done(Some("limited")));
            }
            Filing::Refuse(reason) => {
                if reason == "limited" {
                    crate::metrics::rate_limited("dm_requests");
                }
                // An own invite's conversation already exists here; an ignored row, decided as
                // it is filed, keeps it out of the local member's view without making it a
                // request.
                if read.local
                    && facts.request.is_none()
                    && let (Some(invite), Some(conversation_id)) =
                        (&read.invite, read.conversation_id)
                {
                    let (card_payload, card_signature) = invite
                        .card
                        .as_ref()
                        .map(|c| (c.payload.clone(), c.signature.clone()))
                        .unwrap_or_default();
                    repos
                        .relay_repo
                        .insert_request(NewDmRequest {
                            status: DmRequestStatus::Ignored,
                            user_id,
                            from_principal_id: invite.from_principal_id.clone(),
                            card_payload,
                            card_signature,
                            host_server_id: peer_server_id.to_string(),
                            conversation_id,
                            expires_at: now + IGNORED_REQUEST_TTL,
                        })
                        .await?;
                }
                return Ok(Filed::Done(Some(reason)));
            }
            Filing::Drop => return Ok(Filed::Done(None)),
            // Local members read a conversation they accepted live, not from the queue.
            Filing::Deliver if read.local => return Ok(Filed::Done(None)),
            Filing::Deliver => None,
            Filing::Hold(id) => Some(id),
            filing @ (Filing::FileAccepted | Filing::FileRequest) => {
                let (Some(invite), Some(conversation_id)) = (&read.invite, read.conversation_id)
                else {
                    return Ok(Filed::Done(Some("invalid_envelope")));
                };
                let Some(card) = &invite.card else {
                    return Ok(Filed::Done(Some("invalid_envelope")));
                };
                let pending = filing == Filing::FileRequest;
                // Only a request is listed with its sender's card; a row accepted as it is
                // filed is never read for one.
                let (card_payload, card_signature) = if pending {
                    (card.payload.clone(), card.signature.clone())
                } else {
                    (String::new(), String::new())
                };
                let request = repos
                    .relay_repo
                    .insert_request(NewDmRequest {
                        status: if pending {
                            DmRequestStatus::Pending
                        } else {
                            DmRequestStatus::Accepted
                        },
                        user_id,
                        from_principal_id: invite.from_principal_id.clone(),
                        card_payload,
                        card_signature,
                        host_server_id: peer_server_id.to_string(),
                        conversation_id,
                        expires_at: if pending {
                            now + self.settings.retention
                        } else {
                            accepted_expiry(now)
                        },
                    })
                    .await?;
                if !pending && read.local {
                    return Ok(Filed::Done(None));
                }
                if pending {
                    notices.push(RelayNotice::DmRequestReceived {
                        user_id,
                        request_id: request.id,
                    });
                    Some(request.id)
                } else {
                    None
                }
            }
        };
        Ok(Filed::Queue(request_id))
    }

    /// Whether `host` at `host_origin` speaks for the sender of an invite it delivered. The
    /// card must verify against the sender's key log: the one stored here, else a cached
    /// identity younger than a day, else the identity the host serves for them. The sender's
    /// newest card (the stored, the cached or served, or the invite's) must then list the
    /// host's origin, unless the sender is a local user holding an active subscription at the
    /// host.
    ///
    /// Returns `Unverified` when the host serves no identity for a sender held and cached
    /// nowhere here; `Refused("invalid_invite")` when the invite has no card or names no
    /// principal, the card does not verify against a log obtained (the held one included when
    /// the host serves none), or the newest card does not
    /// list the host; and `Refused("limited")` when the identity would be fetched for a host
    /// past [`MAX_REQUESTS_PER_HOST_PER_DAY`].
    ///
    /// # Errors
    /// What the repositories return.
    async fn invite_sender(
        &self,
        repos: &RelayRepos,
        host: &str,
        host_origin: &str,
        invite: &Invite,
        filed_by_host_today: i64,
    ) -> ServiceResult<InviteSender> {
        const INVALID: InviteSender = InviteSender::Refused("invalid_invite");
        let from = invite.from_principal_id.as_str();
        let Some(card) = &invite.card else {
            return Ok(INVALID);
        };
        if !is_principal_id(from) {
            return Ok(INVALID);
        }
        let mut cards: Vec<(u64, Vec<String>)> = Vec::new();
        let mut verified = None;
        let mut subscribed = false;
        let held = repos.host_repo.find_user(from).await?;
        if let Some((user_id, kind)) = held {
            let log: Vec<SignedJson> = repos
                .key_identity_repo
                .list_key_log(user_id)
                .await?
                .into_iter()
                .map(|e| SignedJson {
                    payload: e.payload,
                    signature: e.signature,
                })
                .collect();
            verified = card_of(from, log, card);
            if let Some(stored) = repos.key_identity_repo.get_profile_card(user_id).await? {
                cards.push((u64::from(stored.version), card_relays(&stored.payload)));
            }
            if kind == UserKind::Local {
                if verified.is_none() {
                    return Ok(INVALID);
                }
                subscribed = repos
                    .subscription_repo
                    .subscribed_hosts(user_id, Utc::now())
                    .await?
                    .iter()
                    .any(|h| h == host);
            }
        }
        if verified.is_none() {
            let cached = repos
                .relay_repo
                .cached_identity(from)
                .await?
                .filter(|c| Utc::now() - c.fetched_at < IDENTITY_CACHE_TTL)
                .and_then(|c| serde_json::from_str::<IdentityDocument>(&c.identity).ok())
                .filter(|d| d.principal_id == from);
            let document = match cached {
                Some(document) => document,
                None if filed_by_host_today >= MAX_REQUESTS_PER_HOST_PER_DAY => {
                    return Ok(InviteSender::Refused("limited"));
                }
                None => match self.fetch_identity(host, from).await {
                    Ok(document) => document,
                    // A log held here that the card failed is checked, not missing.
                    Err(_) if held.is_some() => return Ok(INVALID),
                    Err(_) => return Ok(InviteSender::Unverified),
                },
            };
            if let Some(served) = verified_card(&document) {
                cards.push((served.version, served.relays));
            }
            verified = card_of(from, document.key_log, card);
        }
        let Some(card) = verified else {
            return Ok(INVALID);
        };
        if subscribed {
            return Ok(InviteSender::Verified);
        }
        cards.push((card.version, card.relays));
        let names_host = cards
            .into_iter()
            .max_by_key(|(version, _)| *version)
            .is_some_and(|(_, relays)| relays_include_origin(&relays, host_origin));
        Ok(if names_host {
            InviteSender::Verified
        } else {
            INVALID
        })
    }

    /// Record what an item for `principal_id` says about their `request` for its conversation.
    /// The host's removal of the user from the conversation deletes an accepted row; otherwise
    /// an item the row `delivered` moves an accepted row's expiry out, and an event of the host
    /// on the user's own stream gives the row the id the host gives them.
    async fn note_activity(
        repos: &RelayRepos,
        request: &DmRequest,
        read: &ReadEnvelope,
        principal_id: &str,
        delivered: bool,
        now: DateTime<Utc>,
    ) -> ServiceResult<()> {
        let accepted = request.status == DmRequestStatus::Accepted;
        if accepted
            && read.removes_member
            && read.user_id.is_some()
            && read.user_id == request.host_user_id
        {
            repos
                .relay_repo
                .delete_accepted_request(
                    request.user_id,
                    &request.host_server_id,
                    request.conversation_id,
                )
                .await?;
            info!(
                user_id = %request.user_id,
                host_server_id = %request.host_server_id,
                conversation_id = %request.conversation_id,
                "the host removed a user from an accepted conversation; its row is deleted"
            );
            return Ok(());
        }
        let expires_at = (accepted
            && delivered
            && request.expires_at < now + ACCEPTED_REQUEST_TTL - ACCEPTED_REFRESH_AFTER)
            .then(|| accepted_expiry(now));
        let host_user_id = if request.host_user_id.is_none()
            && read.stream.strip_prefix("u:") == Some(principal_id)
        {
            read.user_id
        } else {
            None
        };
        if expires_at.is_some() || host_user_id.is_some() {
            repos
                .relay_repo
                .refresh_request(request.id, expires_at, host_user_id)
                .await?;
        }
        Ok(())
    }

    /// Drop a user's oldest queued items over the byte cap. Held items are neither counted nor
    /// dropped: their own caps bound them.
    async fn enforce_cap(
        &self,
        repos: &RelayRepos,
        user_id: UserId,
        notices: &mut Vec<RelayNotice>,
    ) -> ServiceResult<u64> {
        let queued = repos.relay_repo.queued_bytes(user_id).await?;
        let over = queued - self.settings.queue_bytes_per_user;
        if over <= 0 {
            return Ok(0);
        }
        let dropped = repos.relay_repo.drop_oldest(user_id, over).await?;
        let removed_items = dropped.iter().map(|space| space.removed_items).sum();
        if !dropped.is_empty() {
            warn!(user_id = %user_id, spaces = dropped.len(), "relay queue over its cap; dropped the oldest items");
            notices.push(RelayNotice::QueueOverflow {
                user_id,
                spaces: dropped
                    .into_iter()
                    .map(|d| QueueSpace {
                        host_server_id: d.host_server_id,
                        workspace_id: d.workspace_id,
                    })
                    .collect(),
            });
        }
        Ok(removed_items)
    }

    /// The identity a relay serves for a principal, fetched over S2S and verified.
    async fn fetch_identity(
        &self,
        relay_server_id: &str,
        principal_id: &str,
    ) -> ServiceResult<IdentityDocument> {
        let response = self
            .client
            .send(S2sRequest {
                peer_server_id: relay_server_id.to_string(),
                method: S2sMethod::Get,
                path: format!("/principals/{principal_id}"),
                body: Vec::new(),
                content_type: None,
                headers: Vec::new(),
            })
            .await
            .map_err(|e| {
                warn!(peer_server_id = %relay_server_id, error = %e, "identity fetch failed");
                upstream(relay_server_id)
            })?;
        match response.status {
            200 => {}
            404 => return Err(ServiceError::NotFound),
            _ => return Err(upstream(relay_server_id)),
        }
        let document: IdentityDocument =
            serde_json::from_slice(&response.body).map_err(|_| upstream(relay_server_id))?;
        if document.principal_id != principal_id {
            return Err(upstream(relay_server_id));
        }
        verify_identity(&document).map_err(|reason| {
            warn!(peer_server_id = %relay_server_id, reason, "a relay served an identity that does not verify");
            upstream(relay_server_id)
        })?;
        Ok(document)
    }
}

/// Fetch a principal's identity from the first of `relay_urls` that serves it, pinning each
/// relay first and skipping one whose server id `admit` refuses. Returns the document and the
/// serving relay's id.
pub(crate) async fn fetch_from_relays(
    peers: &dyn PeerService,
    client: &dyn S2sClient,
    principal_id: &str,
    relay_urls: &[String],
    admit: &(dyn Fn(&str) -> bool + Sync),
) -> ServiceResult<(IdentityDocument, String)> {
    let mut last = ServiceError::NotFound;
    for url in relay_urls {
        let Some(origin) = origin_of(url) else {
            continue;
        };
        let pinned = match peers.pin(&origin, None).await {
            Ok(pinned) => pinned,
            Err(e) => {
                warn!(origin = %origin, error = %e, "could not pin a relay");
                last = ServiceError::upstream_origin(&origin);
                continue;
            }
        };
        if !admit(&pinned.server_id) {
            last = policy::outbound_refused();
            continue;
        }
        let response = client
            .send(S2sRequest {
                peer_server_id: pinned.server_id.clone(),
                method: S2sMethod::Get,
                path: format!("/principals/{principal_id}"),
                body: Vec::new(),
                content_type: None,
                headers: Vec::new(),
            })
            .await;
        let response = match response {
            Ok(r) if r.status == 200 => r,
            Ok(r) if r.status == 404 => {
                last = ServiceError::NotFound;
                continue;
            }
            Ok(_) | Err(_) => {
                last = upstream(&pinned.server_id);
                continue;
            }
        };
        let Ok(document) = serde_json::from_slice::<IdentityDocument>(&response.body) else {
            last = upstream(&pinned.server_id);
            continue;
        };
        if document.principal_id != principal_id || verify_identity(&document).is_err() {
            warn!(peer_server_id = %pinned.server_id, "a relay served an identity that does not verify");
            last = upstream(&pinned.server_id);
            continue;
        }
        return Ok((document, pinned.server_id));
    }
    Err(last)
}

#[async_trait]
impl RelayService for DefaultRelayService {
    async fn contacts(&self, actor: Actor) -> ServiceResult<Vec<ContactDto>> {
        Ok(self
            .repos
            .relay_repo
            .list_contacts(actor.user_id)
            .await?
            .into_iter()
            .map(|c| ContactDto {
                principal_id: c.principal_id,
                relay_urls: c.relay_urls,
                added_at: c.added_at,
            })
            .collect())
    }

    #[instrument(name = "relay.add_contact", skip_all, fields(actor_id = %actor.user_id))]
    async fn add_contact(
        &self,
        actor: Actor,
        principal_id: String,
        relay_urls: Vec<String>,
    ) -> ServiceResult<ContactDto> {
        check_principal(&principal_id)?;
        check_relay_urls(&relay_urls)?;
        let user = self
            .repos
            .user_repo
            .get_by_id(actor.user_id)
            .await?
            .ok_or(ServiceError::Unauthenticated)?;
        if user.principal_id.as_deref() == Some(principal_id.as_str()) {
            return Err(ServiceError::Validation(
                "cannot add yourself as a contact".to_string(),
            ));
        }
        let relay_repo = &self.repos.relay_repo;
        if relay_repo.count_contacts(actor.user_id).await? >= MAX_CONTACTS_PER_USER
            && !relay_repo.is_contact(actor.user_id, &principal_id).await?
        {
            return Err(ServiceError::Validation(format!(
                "a user holds at most {MAX_CONTACTS_PER_USER} contacts; remove one first"
            )));
        }
        let contact = self
            .repos
            .relay_repo
            .upsert_contact(actor.user_id, &principal_id, relay_urls)
            .await?;
        Ok(ContactDto {
            principal_id: contact.principal_id,
            relay_urls: contact.relay_urls,
            added_at: contact.added_at,
        })
    }

    async fn remove_contact(&self, actor: Actor, principal_id: String) -> ServiceResult<bool> {
        Ok(self
            .repos
            .relay_repo
            .delete_contact(actor.user_id, &principal_id)
            .await?)
    }

    #[instrument(name = "relay.block_principal", skip_all, fields(actor_id = %actor.user_id))]
    async fn block_principal(&self, actor: Actor, principal_id: String) -> ServiceResult<bool> {
        check_principal(&principal_id)?;
        let (scope, repos) = self.begin().await?;
        let added = repos.relay_repo.block(actor.user_id, &principal_id).await?;
        let ignored = repos
            .relay_repo
            .ignore_pending_from(
                actor.user_id,
                &principal_id,
                Utc::now() + IGNORED_REQUEST_TTL,
            )
            .await?;
        Self::commit(scope).await?;
        info!(
            target: "audit",
            event = "relay.principal_blocked",
            scope = "user",
            user_id = %actor.user_id,
            principal_id = %principal_id,
            ignored_requests = ignored,
            "a user blocked a principal"
        );
        Ok(added)
    }

    async fn unblock_principal(&self, actor: Actor, principal_id: String) -> ServiceResult<bool> {
        Ok(self
            .repos
            .relay_repo
            .unblock(actor.user_id, &principal_id)
            .await?)
    }

    async fn blocked_principals(&self, actor: Actor) -> ServiceResult<Vec<BlockedPrincipalDto>> {
        Ok(self
            .repos
            .relay_repo
            .list_blocks(actor.user_id)
            .await?
            .into_iter()
            .map(|b| BlockedPrincipalDto {
                principal_id: b.principal_id,
                user_id: b.user_id,
                username: b.username,
                display_name: b.display_name,
                blocked_at: b.blocked_at,
            })
            .collect())
    }

    async fn dm_requests(
        &self,
        actor: Actor,
        status: Option<DmRequestStatus>,
    ) -> ServiceResult<Vec<DmRequestDto>> {
        let requests = self
            .repos
            .relay_repo
            .list_requests(actor.user_id, status)
            .await?;
        let mut result = Vec::with_capacity(requests.len());
        for request in requests {
            result.push(self.request_dto(request).await?);
        }
        Ok(result)
    }

    #[instrument(name = "relay.accept_dm_request", skip_all, fields(actor_id = %actor.user_id))]
    async fn accept_dm_request(
        &self,
        actor: Actor,
        id: DmRequestId,
    ) -> ServiceResult<DmRequestDto> {
        let request = self.own_request(actor, id).await?;
        let (scope, repos) = self.begin().await?;
        let decided = repos
            .relay_repo
            .decide_request(
                request.id,
                DmRequestStatus::Accepted,
                accepted_expiry(Utc::now()),
            )
            .await?
            .ok_or_else(|| undecided(&request))?;
        repos.relay_repo.release_held(request.id).await?;
        let relays = contact_relays(&request.card_payload);
        if !relays.is_empty() {
            repos
                .relay_repo
                .upsert_contact(actor.user_id, &request.from_principal_id, relays)
                .await?;
        }
        Self::commit(scope).await?;
        self.notify(RelayNotice::Items {
            user_id: actor.user_id,
        });
        self.request_dto(decided).await
    }

    #[instrument(name = "relay.ignore_dm_request", skip_all, fields(actor_id = %actor.user_id))]
    async fn ignore_dm_request(
        &self,
        actor: Actor,
        id: DmRequestId,
    ) -> ServiceResult<DmRequestDto> {
        let request = self.own_request(actor, id).await?;
        let (scope, repos) = self.begin().await?;
        let decided = repos
            .relay_repo
            .decide_request(
                request.id,
                DmRequestStatus::Ignored,
                Utc::now() + IGNORED_REQUEST_TTL,
            )
            .await?
            .ok_or_else(|| undecided(&request))?;
        repos.relay_repo.delete_held(request.id).await?;
        Self::commit(scope).await?;
        self.request_dto(decided).await
    }

    #[instrument(name = "relay.report_dm_request", skip_all, fields(actor_id = %actor.user_id))]
    async fn report_dm_request(
        &self,
        actor: Actor,
        id: DmRequestId,
        reason: String,
    ) -> ServiceResult<bool> {
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > MAX_REPORT_REASON_CHARS {
            return Err(ServiceError::Validation(format!(
                "a reason is 1 to {MAX_REPORT_REASON_CHARS} characters"
            )));
        }
        let request = self.own_request(actor, id).await?;
        if request.status == DmRequestStatus::Pending {
            self.ignore_dm_request(actor, id).await?;
        }
        let report = match self
            .repos
            .relay_abuse_repo
            .insert_report(NewAbuseReport {
                reporter_user_id: Some(actor.user_id),
                subject_principal_id: Some(request.from_principal_id.clone()),
                subject_server_id: Some(request.host_server_id.clone()),
                request_id: Some(request.id),
                reason: reason.to_string(),
            })
            .await
        {
            Ok(report) => report,
            Err(e) if matches!(e.kind(), RepoErrorKind::Conflict) => {
                return Err(ServiceError::Conflict(
                    "you already reported this request".to_string(),
                ));
            }
            Err(e) => return Err(e.into()),
        };
        info!(
            target: "audit",
            event = "relay.dm_request_reported",
            user_id = %actor.user_id,
            report_id = %report.id,
            request_id = %request.id,
            principal_id = %request.from_principal_id,
            host_server_id = %request.host_server_id,
            "a user reported a DM request"
        );
        Ok(true)
    }

    async fn relay_items(
        &self,
        caller: MlsCaller,
        after: Option<i64>,
        limit: i64,
    ) -> ServiceResult<RelayItemPageDto> {
        let device = self.session_device(&caller).await?;
        let after = match after {
            Some(after) => after,
            None => self.repos.relay_repo.device_cursor(device).await?,
        };
        let mut items = self
            .repos
            .relay_repo
            .list_items(caller.actor.user_id, device, after, limit + 1)
            .await?;
        let has_more = items.len() as i64 > limit;
        items.truncate(limit.max(0) as usize);
        Ok(RelayItemPageDto {
            items: items.into_iter().map(item_dto).collect(),
            has_more,
        })
    }

    async fn ack_relay_items(&self, caller: MlsCaller, up_to: i64) -> ServiceResult<bool> {
        let device = self.session_device(&caller).await?;
        if up_to < 0 {
            return Err(ServiceError::Validation("upTo is an item id".to_string()));
        }
        self.repos.relay_repo.ack(device, up_to).await?;
        metrics::counter!("rim_relay_ack_requests_total").increment(1);
        Ok(true)
    }

    #[instrument(name = "relay.leave_relay", skip_all, fields(actor_id = %actor.user_id))]
    async fn leave_relay(&self, actor: Actor) -> ServiceResult<bool> {
        self.repos.relay_repo.clear_user(actor.user_id).await?;
        info!(
            target: "audit",
            event = "relay.left",
            user_id = %actor.user_id,
            "a user left this relay; their queue, requests and contacts are deleted"
        );
        Ok(true)
    }

    fn subscribe(&self) -> broadcast::Receiver<RelayNotice> {
        self.notices.subscribe()
    }

    fn publish(&self, notice: RelayNotice) {
        self.notify(notice);
    }

    async fn check_outgoing_request(
        &self,
        user_id: UserId,
        device_key_id: Option<DeviceKeyId>,
        payload: String,
        signature: String,
    ) -> ServiceResult<String> {
        let forbidden = |why: &str| ServiceError::Forbidden(why.to_string());
        let device_key_id =
            device_key_id.ok_or_else(|| forbidden("this session is not bound to a device"))?;
        let statement = SignedStatement { payload, signature };
        let unverified = UnverifiedDeviceRequest::read(&statement)
            .map_err(|e| ServiceError::Validation(e.to_string()))?;
        let user = self
            .repos
            .user_repo
            .get_by_id(user_id)
            .await?
            .ok_or(ServiceError::Unauthenticated)?;
        if user.principal_id.as_deref() != Some(unverified.principal_id.as_str()) {
            return Err(forbidden("the device request names another principal"));
        }
        let device = self
            .repos
            .key_identity_repo
            .get_device(user_id, unverified.device_id)
            .await?
            .filter(|d| d.id == device_key_id)
            .ok_or_else(|| forbidden("the device request names another device"))?;
        if device.revoked_at.is_some() {
            return Err(forbidden("the device is revoked"));
        }
        let key = identity::device_key(&device.public_key)?;
        let request = unverified
            .verify(&key)
            .map_err(|_| forbidden("the device signature does not verify"))?;
        let now = Utc::now();
        for (limiter, decision) in [
            (
                "relay_forward_device",
                self.forwards_per_device
                    .check_and_record(&device.id.to_string(), "forward", now),
            ),
            (
                "relay_forward_user",
                self.forwards_per_user
                    .check_and_record(&user_id.to_string(), now),
            ),
        ] {
            if let RateLimitDecision::Limited {
                retry_after_seconds,
            } = decision
            {
                warn!(user_id = %user_id, device_key_id = %device.id, "a user's forwarded requests are over their quota");
                crate::metrics::rate_limited(limiter);
                return Err(ServiceError::RateLimited {
                    retry_after_seconds,
                });
            }
        }
        Ok(request.host)
    }

    async fn local_identity(
        &self,
        principal_id: String,
    ) -> ServiceResult<Option<IdentityDocument>> {
        if self
            .repos
            .relay_abuse_repo
            .is_principal_blocked(&principal_id)
            .await?
        {
            return Ok(None);
        }
        match self.repos.host_repo.find_user(&principal_id).await? {
            Some((user_id, UserKind::Local)) => {
                let username = self
                    .repos
                    .user_repo
                    .get_by_id(user_id)
                    .await?
                    .map(|u| u.username);
                local_document(
                    self.repos.key_identity_repo.as_ref(),
                    self.repos.e2ee_repo.as_ref(),
                    user_id,
                    &principal_id,
                    username,
                )
                .await
            }
            _ => Ok(None),
        }
    }

    async fn identity_for_peer(
        &self,
        peer_server_id: String,
        principal_id: String,
    ) -> ServiceResult<Option<IdentityDocument>> {
        let Some((user_id, UserKind::Local)) =
            self.repos.host_repo.find_user(&principal_id).await?
        else {
            return Ok(None);
        };
        if !self.admits_peer(user_id, &peer_server_id).await? {
            return Ok(None);
        }
        self.local_identity(principal_id).await
    }

    #[instrument(name = "relay.remote_identity", skip_all, fields(actor_id = %_actor.user_id))]
    async fn remote_identity(
        &self,
        _actor: Actor,
        principal_id: String,
        relay_url: Option<String>,
    ) -> ServiceResult<RemoteIdentityDto> {
        check_principal(&principal_id)?;
        if let Some(document) = self.local_identity(principal_id.clone()).await? {
            return Ok(RemoteIdentityDto {
                document,
                served_by: self.own_id(),
            });
        }
        let cached = self.repos.relay_repo.cached_identity(&principal_id).await?;
        if let Some(cached) = &cached
            && Utc::now() - cached.fetched_at < IDENTITY_CACHE_TTL
            && let Ok(document) = serde_json::from_str::<IdentityDocument>(&cached.identity)
        {
            return Ok(RemoteIdentityDto {
                document,
                served_by: cached.served_by.clone(),
            });
        }
        let (document, served_by) = match relay_url {
            Some(url) => {
                check_relay_urls(std::slice::from_ref(&url))?;
                fetch_from_relays(
                    self.peers.as_ref(),
                    self.client.as_ref(),
                    &principal_id,
                    &[url],
                    &|_| true,
                )
                .await?
            }
            None => {
                let served_by = cached
                    .map(|c| c.served_by)
                    .ok_or_else(|| ServiceError::Validation("relayUrl is required".to_string()))?;
                (
                    self.fetch_identity(&served_by, &principal_id).await?,
                    served_by,
                )
            }
        };
        let text = serde_json::to_string(&document).map_err(ServiceError::internal)?;
        self.repos
            .relay_repo
            .store_identity(&principal_id, &served_by, &text)
            .await?;
        Ok(RemoteIdentityDto {
            document,
            served_by,
        })
    }

    async fn remote_server(&self, _actor: Actor, origin: String) -> ServiceResult<RemoteServerDto> {
        let origin = origin_of(&origin)
            .filter(|o| *o == origin.trim_end_matches('/'))
            .ok_or_else(|| {
                ServiceError::Validation("origin must be scheme://host[:port]".to_string())
            })?;
        let pinned = self.peers.pin(&origin, None).await?;
        let discovery = self
            .peers
            .discovery_document(&pinned.server_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        Ok(RemoteServerDto {
            server_id: pinned.server_id,
            origin: pinned.origin,
            discovery,
        })
    }

    #[instrument(name = "relay.deliver", skip_all, fields(peer_server_id = %peer_server_id, items = items.len()))]
    async fn deliver(
        &self,
        peer_server_id: String,
        items: Vec<DeliverItem>,
    ) -> ServiceResult<DeliverResponse> {
        if items.len() > MAX_DELIVER_ITEMS {
            return Err(ServiceError::Validation(format!(
                "deliver at most {MAX_DELIVER_ITEMS} items"
            )));
        }
        let keys = self.peer_keys(&peer_server_id, None).await?;
        let (scope, repos) = self.begin().await?;
        let mut through = repos.relay_repo.received_through(&peer_server_id).await?;
        let mut refused = Vec::new();
        let mut notices = Vec::new();
        let mut users = HashSet::new();
        for item in &items {
            let id: i64 = item
                .id
                .parse()
                .map_err(|_| ServiceError::Validation("an item id is not a number".to_string()))?;
            if id <= through {
                continue;
            }
            match self
                .read_envelope(&peer_server_id, &keys, &item.envelope)
                .await
            {
                None => refused.push(RefusedItem {
                    id: item.id.clone(),
                    reason: "invalid_envelope".to_string(),
                    principal_id: None,
                }),
                Some(read) => {
                    let mut envelope_id = None;
                    let mut seen = HashSet::new();
                    for principal_id in item.recipients.iter().filter(|p| seen.insert(*p)) {
                        let before = notices.len();
                        if let Some(reason) = self
                            .file_for(
                                &repos,
                                &peer_server_id,
                                &read,
                                &item.envelope,
                                &mut envelope_id,
                                principal_id,
                                item.device_id,
                                &mut notices,
                            )
                            .await?
                        {
                            refused.push(RefusedItem {
                                id: item.id.clone(),
                                reason: reason.to_string(),
                                principal_id: Some(principal_id.clone()),
                            });
                        }
                        for notice in &notices[before..] {
                            users.insert(notice.user_id());
                        }
                    }
                }
            }
            through = id;
        }
        let mut overflow_items = 0;
        for user_id in users {
            overflow_items += self.enforce_cap(&repos, user_id, &mut notices).await?;
        }
        repos
            .relay_repo
            .advance_received_through(&peer_server_id, through)
            .await?;
        Self::commit(scope).await?;
        metrics::counter!("rim_relay_removed_items_total", "reason" => "overflow")
            .increment(overflow_items);
        for notice in notices {
            if matches!(&notice, RelayNotice::Items { .. }) {
                metrics::counter!("rim_relay_enqueued_items_total").increment(1);
            }
            self.notify(notice);
        }
        Ok(DeliverResponse {
            acked_through: through.to_string(),
            refused,
        })
    }

    #[instrument(name = "relay.claim_for_peer", skip_all, fields(peer_server_id = %peer_server_id))]
    async fn claim_for_peer(
        &self,
        peer_server_id: String,
        request: PeerClaimRequest,
    ) -> ServiceResult<PeerClaimResponse> {
        if request.conversation.host != peer_server_id {
            return Err(ServiceError::Forbidden(
                "the claim names another host".to_string(),
            ));
        }
        if request.devices.is_empty() || request.devices.len() > MAX_CLAIM_DEVICES {
            return Err(ServiceError::Validation(format!(
                "claim 1 to {MAX_CLAIM_DEVICES} devices"
            )));
        }
        if request.devices.iter().collect::<HashSet<_>>().len() != request.devices.len() {
            return Err(ServiceError::Validation(
                "a device is named twice".to_string(),
            ));
        }
        let suite = request.cipher_suite.unwrap_or(PREFERRED_CIPHER_SUITE);
        if !ACCEPTED_CIPHER_SUITES.contains(&suite) {
            return Err(ServiceError::Validation(format!(
                "cipher suite {suite} is not accepted"
            )));
        }
        let Some((user_id, UserKind::Local)) = self
            .repos
            .host_repo
            .find_user(&request.principal_id)
            .await?
        else {
            return Err(ServiceError::NotFound);
        };
        if self
            .repos
            .relay_abuse_repo
            .is_principal_blocked(&request.principal_id)
            .await?
        {
            return Err(ServiceError::NotFound);
        }
        if !self.admits_peer(user_id, &peer_server_id).await? {
            return Err(ServiceError::Forbidden(
                "this server takes no DMs from that server".to_string(),
            ));
        }
        let claimer = request.claimer.principal_id.as_str();
        if self.repos.relay_repo.is_blocked(user_id, claimer).await?
            || self
                .repos
                .relay_abuse_repo
                .is_principal_blocked(claimer)
                .await?
        {
            return Err(ServiceError::Forbidden(
                "the claimer is blocked".to_string(),
            ));
        }
        // The claiming server names the claimer and may make them up, so only a tie between the
        // user and that server counts: a conversation it homes they accepted, or a workspace it
        // hosts they belong to.
        let stranger = !(self
            .repos
            .relay_repo
            .accepted_at_host(user_id, &peer_server_id)
            .await?
            || self.subscribed_at(user_id, &peer_server_id).await?);
        if stranger
            && let RateLimitDecision::Limited {
                retry_after_seconds,
            } = self
                .stranger_claims
                .check_and_record(claimer, "stranger_claim", Utc::now())
        {
            crate::metrics::rate_limited("stranger_claim");
            return Err(ServiceError::RateLimited {
                retry_after_seconds,
            });
        }
        let (scope, repos) = self.begin().await?;
        let mut key_packages = Vec::with_capacity(request.devices.len());
        for device_id in &request.devices {
            let Some(device) = repos
                .key_identity_repo
                .get_device(user_id, *device_id)
                .await?
            else {
                continue;
            };
            if device.revoked_at.is_some() {
                continue;
            }
            if let Some(package) = repos
                .e2ee_repo
                .claim_key_package_for_peer(device.id, suite, !stranger)
                .await?
            {
                key_packages.push(PeerKeyPackage {
                    device_id: *device_id,
                    key_package: URL_SAFE_NO_PAD.encode(&package.key_package),
                    key_package_ref: URL_SAFE_NO_PAD.encode(&package.key_package_ref),
                    last_resort: package.last_resort,
                });
            }
        }
        Self::commit(scope).await?;
        info!(
            claimed = key_packages.len(),
            stranger,
            user_id = %user_id,
            "handed out key packages to a host"
        );
        Ok(PeerClaimResponse { key_packages })
    }

    async fn file_local(&self, envelope: SignedJson, recipients: Vec<String>) -> ServiceResult<()> {
        let statement = SignedStatement {
            payload: envelope.payload.clone(),
            signature: envelope.signature.clone(),
        };
        let event = read_host_event(&statement).map_err(ServiceError::internal)?;
        let own_id = self.own_id();
        let read = envelope_fields(event, true).ok_or_else(|| {
            ServiceError::internal(anyhow::anyhow!("an own host event has no space id"))
        })?;
        let (scope, repos) = self.begin().await?;
        let mut notices = Vec::new();
        let mut envelope_id = None;
        for principal_id in &recipients {
            if let Some(reason) = self
                .file_for(
                    &repos,
                    &own_id,
                    &read,
                    &envelope,
                    &mut envelope_id,
                    principal_id,
                    None,
                    &mut notices,
                )
                .await?
            {
                info!(principal_id = %principal_id, reason, stream = %read.stream, "the request inbox refused an own event for a local member");
            }
        }
        Self::commit(scope).await?;
        for notice in notices {
            if matches!(&notice, RelayNotice::Items { .. }) {
                metrics::counter!("rim_relay_enqueued_items_total").increment(1);
            }
            self.notify(notice);
        }
        Ok(())
    }

    async fn conversation_hidden(
        &self,
        user_id: UserId,
        conversation_id: Uuid,
    ) -> ServiceResult<bool> {
        Ok(self
            .repos
            .relay_repo
            .conversation_hidden(user_id, conversation_id)
            .await?)
    }

    async fn request_pending(&self, user_id: UserId, conversation_id: Uuid) -> ServiceResult<bool> {
        Ok(self
            .repos
            .relay_repo
            .request_pending(user_id, conversation_id)
            .await?)
    }

    async fn pending_holders(&self, conversation_id: Uuid) -> ServiceResult<Vec<String>> {
        Ok(self
            .repos
            .relay_repo
            .pending_holders(conversation_id)
            .await?
            .into_iter()
            .map(|(_, principal_id)| principal_id)
            .collect())
    }

    async fn pull_outbox(&self, peer_server_id: String) -> ServiceResult<u64> {
        let after = self
            .repos
            .relay_repo
            .received_through(&peer_server_id)
            .await?;
        let response = self
            .client
            .send(S2sRequest {
                peer_server_id: peer_server_id.clone(),
                method: S2sMethod::Get,
                path: format!("/outbox?after={after}&limit={MAX_DELIVER_ITEMS}"),
                body: Vec::new(),
                content_type: None,
                headers: Vec::new(),
            })
            .await
            .map_err(|_| upstream(&peer_server_id))?;
        if response.status != 200 {
            return Err(upstream(&peer_server_id));
        }
        let page: OutboxPage =
            serde_json::from_slice(&response.body).map_err(|_| upstream(&peer_server_id))?;
        let count = page.items.len() as u64;
        if count > 0 {
            self.deliver(peer_server_id, page.items).await?;
        }
        Ok(count)
    }

    async fn identity_changed(&self, user_id: UserId) -> ServiceResult<()> {
        let Some(user) = self.repos.user_repo.get_by_id(user_id).await? else {
            return Ok(());
        };
        let Some(principal_id) = user.principal_id else {
            return Ok(());
        };
        let Some(document) = self.local_identity(principal_id.clone()).await? else {
            return Ok(());
        };
        let body = serde_json::to_vec(&document).map_err(ServiceError::internal)?;
        let own = self.own_id();
        for host in self.repos.relay_repo.hosts_of_user(user_id).await? {
            if host == own {
                continue;
            }
            let result = self
                .client
                .send(S2sRequest {
                    peer_server_id: host.clone(),
                    method: S2sMethod::Post,
                    path: "/identity".to_string(),
                    body: body.clone(),
                    content_type: None,
                    headers: Vec::new(),
                })
                .await;
            match result {
                Ok(r) if r.status == 200 => {}
                Ok(r) => {
                    warn!(peer_server_id = %host, status = r.status, "a host refused an identity push")
                }
                Err(e) => {
                    warn!(peer_server_id = %host, error = %e, "could not push an identity to a host")
                }
            }
        }
        Ok(())
    }

    async fn cleanup(&self) -> ServiceResult<RelayCleanup> {
        let now = Utc::now();
        let acked_items = self.repos.relay_repo.delete_acked_items().await?;
        metrics::counter!("rim_relay_removed_items_total", "reason" => "acknowledged")
            .increment(acked_items);
        let expired_items = self.repos.relay_repo.delete_expired_items(now).await?;
        metrics::counter!("rim_relay_removed_items_total", "reason" => "expired")
            .increment(expired_items);
        Ok(RelayCleanup {
            acked_items,
            expired_items,
            expired_requests: self.repos.relay_repo.delete_expired_requests(now).await?,
            orphan_envelopes: self.repos.relay_repo.delete_orphan_envelopes().await?,
        })
    }
}

#[cfg(test)]
mod tests {
    use rim_services::s2s::dto::{PeerDto, S2sResponse};
    use rim_services::s2s::service::{MockPeerService, MockS2sClient};

    use super::*;

    #[test]
    fn relay_urls_follow_the_contact_link_rules() {
        assert!(check_relay_urls(&["https://relay.example".to_string()]).is_ok());
        assert!(check_relay_urls(&[]).is_err());
        assert!(check_relay_urls(&["https://a.example/?x".to_string()]).is_err());
        assert!(check_relay_urls(&["https://u@a.example".to_string()]).is_err());
        assert!(check_relay_urls(&["https://a.example/#f".to_string()]).is_err());
        assert!(check_relay_urls(&["https://a b".to_string()]).is_err());
        assert!(check_relay_urls(&["x".repeat(257)]).is_err());
        assert!(check_relay_urls(&vec!["https://a.example".to_string(); 9]).is_err());
    }

    #[test]
    fn an_expired_request_is_not_found_and_a_live_one_was_decided_already() {
        let request = |expires_at| DmRequest {
            id: DmRequestId::new(),
            user_id: UserId::new(),
            from_principal_id: "rimp1_alice".to_string(),
            card_payload: "{}".to_string(),
            card_signature: "ed25519:c".to_string(),
            host_server_id: "rims1_host".to_string(),
            conversation_id: Uuid::now_v7(),
            status: DmRequestStatus::Pending,
            created_at: Utc::now(),
            decided_at: None,
            expires_at,
            host_user_id: None,
        };
        assert!(matches!(
            undecided(&request(Utc::now() - Duration::minutes(1))),
            ServiceError::NotFound
        ));
        assert!(matches!(
            undecided(&request(Utc::now() + Duration::days(1))),
            ServiceError::Conflict(_)
        ));
    }

    fn peer(server_id: &str, origin: &str) -> PeerDto {
        PeerDto {
            server_id: server_id.to_string(),
            origin: origin.to_string(),
            status: "active".to_string(),
            head_seq: 0,
            descriptor_version: 1,
            descriptor_payload: "{}".to_string(),
            fetched_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn a_relay_that_cannot_be_pinned_is_named_by_its_origin() {
        let mut peers = MockPeerService::new();
        peers.expect_pin().returning(|_, _| {
            Box::pin(async { Err(ServiceError::Validation("no document".to_string())) })
        });
        let mut client = MockS2sClient::new();
        client.expect_send().never();
        let err = fetch_from_relays(
            &peers,
            &client,
            "rimp1_alice",
            &["https://relay.example/x".to_string()],
            &|_| true,
        )
        .await
        .expect_err("unpinned");
        assert!(
            matches!(
                &err,
                ServiceError::UpstreamUnavailable { host_server_id: None, origin: Some(o) }
                    if o == "https://relay.example"
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn a_pinned_relay_that_fails_is_named_by_its_server_id() {
        let mut peers = MockPeerService::new();
        peers.expect_pin().returning(|origin, _| {
            let pinned = peer("rims1_relay", origin);
            Box::pin(async move { Ok(pinned) })
        });
        let mut client = MockS2sClient::new();
        client.expect_send().returning(|_| {
            Box::pin(async {
                Ok(S2sResponse {
                    status: 500,
                    content_type: "application/json".to_string(),
                    body: Vec::new(),
                    key_seq: 0,
                    headers: Vec::new(),
                })
            })
        });
        let err = fetch_from_relays(
            &peers,
            &client,
            "rimp1_alice",
            &["https://relay.example".to_string()],
            &|_| true,
        )
        .await
        .expect_err("refused");
        assert!(
            matches!(
                &err,
                ServiceError::UpstreamUnavailable { host_server_id: Some(id), origin: None }
                    if id == "rims1_relay"
            ),
            "{err:?}"
        );
    }

    #[test]
    fn origins_drop_paths_and_keep_ports() {
        assert_eq!(
            origin_of("https://relay.example/x").as_deref(),
            Some("https://relay.example")
        );
        assert_eq!(
            origin_of("http://127.0.0.1:8099").as_deref(),
            Some("http://127.0.0.1:8099")
        );
        assert_eq!(origin_of("ftp://relay.example"), None);
    }

    #[test]
    fn contact_relays_keep_valid_urls_only() {
        assert_eq!(
            contact_relays(r#"{"relays":["https://a.example"]}"#),
            vec!["https://a.example"]
        );
        assert!(contact_relays(r#"{"relays":["https://a.example/?x"]}"#).is_empty());
        assert!(contact_relays("{}").is_empty());
    }

    #[test]
    fn card_relays_are_read_from_the_payload() {
        assert_eq!(
            card_relays(r#"{"relays":["https://a.example","https://b.example"]}"#),
            vec!["https://a.example", "https://b.example"]
        );
        assert!(card_relays("{}").is_empty());
    }
}
