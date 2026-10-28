//! Host events for relays: the writer turns each workspace event into signed envelopes on the
//! streams its remote recipients see and appends one outbox row per relay (for a workspace,
//! each relay holding an active delegation of a recipient, and for the decision on a join
//! request of a person of another server, the relay that filed it; for the direct space, the
//! relays a recipient's card lists, those the operator's outbound DM setting admits); it files
//! what local members get through the request inbox: invites and held events of the direct
//! space and of workspace DMs. The sender delivers each relay's rows in order with backoff, and asks the
//! writer to send a conversation's invite again to a member whose relay no longer holds it. A
//! relay's rows stop being queued while its backlog is at the per-relay caps, or while it is
//! dead: every delivery to it has failed for the configured period.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, watch};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use rim_core::conversation::model::{Conversation, ConversationType};
use rim_core::host::model::{RemoteMember, UserKind};
use rim_core::host::repository::HostRepository;
use rim_core::server::model::ServerId;
use rim_core::user::model::{User, UserId};
use rim_identity::SignedStatement;
use rim_infra_db::begin_transaction;
use rim_s2s::SharedIdentity;
use rim_s2s::host::{HostEventFields, read_host_event, sign_host_event};
use rim_services::default_workspace::DefaultWorkspaceService;
use rim_services::dm_policy::dto::DmOperatorPolicy;
use rim_services::events::visibility::user_can_see_event_as;
use rim_services::events::{Broadcaster, WorkspaceEvent};
use rim_services::relay::dto::{
    DeliverItem, DeliverRequest, DeliverResponse, RefusedItem, SignedJson,
};
use rim_services::relay::service::RelayService;
use rim_services::s2s::dto::{S2sMethod, S2sRequest};
use rim_services::s2s::service::S2sClient;
use rim_services::{ServiceError, ServiceResult};

use crate::host::{DefaultHostService, HostRepos};
use crate::metrics;
use crate::relay::policy::{DmPolicy, outbound};
use crate::relay::{MAX_CONTACTS_PER_USER, MAX_DELIVER_BYTES, MAX_DELIVER_ITEMS, contact_relays};
use crate::repos::AppRepos;

/// Largest host event payload.
const MAX_EVENT_BYTES: usize = 512 * 1024;
/// How long a relay's refusal of a principal keeps the writer from addressing them.
const REFUSAL_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
/// How often a conversation's invite is sent again to one member whose relay refuses its items.
const REINVITE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
/// Longest wait between delivery attempts.
const MAX_BACKOFF_SECS: i64 = 300;
/// Most outbox rows kept for one relay above what it acknowledged.
const MAX_PEER_OUTBOX_ROWS: i64 = 100_000;
/// Most outbox bytes kept for one relay above what it acknowledged: 2 GiB.
const MAX_PEER_OUTBOX_BYTES: i64 = 2 * 1024 * 1024 * 1024;
/// How long the writer counts on a relay's backlog as read before reading it again.
const BACKLOG_TTL: std::time::Duration = std::time::Duration::from_secs(10);
/// Least time between two warnings about rows not queued for one relay.
const DROP_WARN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// The stream an event belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Route {
    /// `c:<conversationId>`.
    Conversation(Uuid),
    /// `u:<principalId>`, one envelope per recipient.
    User,
    /// Directory events: `w` for members, `u:` for a guest about themselves.
    Directory,
    /// `w`, to members who are not guests.
    Workspace,
    /// `e`, without a `seq`.
    Ephemeral,
}

/// Which stream an event goes on.
pub(crate) fn route(event: &WorkspaceEvent, event_json: &Value) -> Route {
    match event {
        WorkspaceEvent::NotificationDue { .. }
        | WorkspaceEvent::UserMentioned { .. }
        | WorkspaceEvent::ReadStateUpdated { .. }
        | WorkspaceEvent::MlsWelcomeAvailable { .. }
        | WorkspaceEvent::JoinRequestCreated { .. }
        | WorkspaceEvent::JoinRequestDecided { .. } => Route::User,
        WorkspaceEvent::MemberProfileUpdated { .. }
        | WorkspaceEvent::MemberBanChanged { .. }
        | WorkspaceEvent::MemberJoined { .. }
        | WorkspaceEvent::MemberLeft { .. }
        | WorkspaceEvent::MemberRoleChanged { .. } => Route::Directory,
        WorkspaceEvent::TypingStarted { .. } | WorkspaceEvent::PresenceChanged { .. } => {
            Route::Ephemeral
        }
        _ => match event_json
            .get("conversation_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
        {
            Some(id) => Route::Conversation(id),
            None => Route::Workspace,
        },
    }
}

/// One envelope to sign: its stream, the remote members it reaches, and the device it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Planned {
    pub stream: String,
    pub recipients: Vec<RemoteMember>,
    pub device_id: Option<Uuid>,
    /// Local members, by principal id, whose request inbox files the envelope.
    pub local: Vec<String>,
}

fn user_stream(member: &RemoteMember) -> String {
    format!("u:{}", member.principal_id)
}

/// The envelopes an event becomes for a space's remote members, by the visibility rules
/// local subscribers get.
pub(crate) fn plan(
    event: &WorkspaceEvent,
    event_json: &Value,
    members: &[RemoteMember],
) -> Vec<Planned> {
    let sees =
        |m: &RemoteMember| user_can_see_event_as(event, &m.user_id.to_string(), m.role.is_guest());
    let visible: Vec<RemoteMember> = members.iter().filter(|m| sees(m)).cloned().collect();
    if visible.is_empty() {
        return Vec::new();
    }
    let device_id = match event {
        WorkspaceEvent::MlsWelcomeAvailable { device_id, .. } => Some(*device_id),
        _ => None,
    };
    let together = |stream: String, recipients: Vec<RemoteMember>| {
        if recipients.is_empty() {
            Vec::new()
        } else {
            vec![Planned {
                stream,
                recipients,
                device_id: None,
                local: Vec::new(),
            }]
        }
    };
    match route(event, event_json) {
        Route::User => visible
            .into_iter()
            .map(|m| Planned {
                stream: user_stream(&m),
                recipients: vec![m],
                device_id,
                local: Vec::new(),
            })
            .collect(),
        Route::Directory => {
            let (guests, members): (Vec<_>, Vec<_>) =
                visible.into_iter().partition(|m| m.role.is_guest() || matches!(event, WorkspaceEvent::MemberLeft { user_id, .. } if *user_id == m.user_id));
            let mut planned = together("w".to_string(), members);
            planned.extend(guests.into_iter().map(|m| Planned {
                stream: user_stream(&m),
                recipients: vec![m],
                device_id: None,
                local: Vec::new(),
            }));
            planned
        }
        Route::Workspace => together(
            "w".to_string(),
            visible.into_iter().filter(|m| !m.role.is_guest()).collect(),
        ),
        Route::Ephemeral => together("e".to_string(), visible),
        Route::Conversation(id) => together(format!("c:{id}"), visible),
    }
}

/// An outbox row's item as delivered: its stored JSON with the outbox id added.
pub(crate) fn deliver_item(id: i64, item: &str) -> Result<DeliverItem, serde_json::Error> {
    let mut value: Value = serde_json::from_str(item)?;
    if let Some(object) = value.as_object_mut() {
        object.insert("id".to_string(), Value::from(id.to_string()));
    }
    serde_json::from_value(value)
}

/// `(relay, principal)` pairs a relay does not hold, skipped by the writer for 24 hours.
#[derive(Default)]
pub struct RefusalCache {
    refused: Mutex<HashMap<(String, String), Instant>>,
}

impl RefusalCache {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), Instant>> {
        match self.refused.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn refuse(&self, relay: &str, principal_id: &str) {
        self.lock().insert(
            (relay.to_string(), principal_id.to_string()),
            Instant::now(),
        );
    }

    pub fn is_refused(&self, relay: &str, principal_id: &str) -> bool {
        let mut refused = self.lock();
        refused.retain(|_, at| at.elapsed() < REFUSAL_TTL);
        refused.contains_key(&(relay.to_string(), principal_id.to_string()))
    }
}

/// Why a row was not queued for a relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotQueued {
    /// The relay's backlog is at [`MAX_PEER_OUTBOX_ROWS`] or [`MAX_PEER_OUTBOX_BYTES`].
    PeerCap,
    /// Every delivery to the relay has failed for the dead-peer period.
    DeadPeer,
}

impl NotQueued {
    /// The `reason` label of the metric counting such rows.
    fn label(self) -> &'static str {
        match self {
            Self::PeerCap => "peer_cap",
            Self::DeadPeer => "dead_peer",
        }
    }
}

/// One relay's backlog as the writer knows it.
struct Backlog {
    rows: i64,
    bytes: i64,
    dead: bool,
    read_at: Instant,
    /// Rows not queued since the last warning.
    not_queued: u64,
    warned_at: Option<Instant>,
}

/// Each relay's outbox backlog as the writer knows it: read from the database at most every
/// 10 seconds and counted forward with the rows the writer adds. The writer asks it before
/// queueing a row; the sender drops a relay's copy when a delivery to it succeeds after
/// failures, so queueing resumes at once.
pub struct PeerBacklogs {
    dead_after: Duration,
    peers: Mutex<HashMap<String, Backlog>>,
}

impl PeerBacklogs {
    /// Backlogs of relays that are dead once every delivery to them has failed for
    /// `dead_after`.
    pub fn new(dead_after: Duration) -> Self {
        Self {
            dead_after,
            peers: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Backlog>> {
        match self.peers.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Whether a relay whose deliveries have failed since `failing_since` is dead at `now`.
    pub(crate) fn is_dead(&self, failing_since: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
        failing_since.is_some_and(|since| now - since >= self.dead_after)
    }

    /// Whether one more row may be queued for `peer`: not while it is dead, nor while its
    /// backlog is at either cap. A row refused is counted, and warned about at most once a
    /// minute per relay.
    ///
    /// # Errors
    /// When the backlog has to be read and cannot be.
    pub(crate) async fn admit(&self, repo: &dyn HostRepository, peer: &str) -> ServiceResult<bool> {
        let fresh = self
            .lock()
            .get(peer)
            .is_some_and(|b| b.read_at.elapsed() < BACKLOG_TTL);
        if !fresh {
            let backlog = repo.outbox_backlog(peer).await?;
            let dead = self.is_dead(backlog.failing_since, Utc::now());
            let mut peers = self.lock();
            let entry = peers.entry(peer.to_string()).or_insert(Backlog {
                rows: 0,
                bytes: 0,
                dead: false,
                read_at: Instant::now(),
                not_queued: 0,
                warned_at: None,
            });
            entry.rows = backlog.rows;
            entry.bytes = backlog.bytes;
            entry.dead = dead;
            entry.read_at = Instant::now();
        }
        let mut peers = self.lock();
        let Some(entry) = peers.get_mut(peer) else {
            return Ok(true);
        };
        let refusal = if entry.dead {
            NotQueued::DeadPeer
        } else if entry.rows >= MAX_PEER_OUTBOX_ROWS || entry.bytes >= MAX_PEER_OUTBOX_BYTES {
            NotQueued::PeerCap
        } else {
            return Ok(true);
        };
        metrics::outbox_not_queued(refusal.label());
        entry.not_queued += 1;
        if entry
            .warned_at
            .is_none_or(|at| at.elapsed() >= DROP_WARN_INTERVAL)
        {
            warn!(
                peer_server_id = %peer,
                reason = refusal.label(),
                rows = entry.rows,
                bytes = entry.bytes,
                not_queued = entry.not_queued,
                "host events for a relay are not queued"
            );
            entry.not_queued = 0;
            entry.warned_at = Some(Instant::now());
        }
        Ok(false)
    }

    /// Count a row of `bytes` queued for `peer`.
    pub(crate) fn queued(&self, peer: &str, bytes: i64) {
        if let Some(entry) = self.lock().get_mut(peer) {
            entry.rows += 1;
            entry.bytes += bytes;
        }
    }

    /// Forget `peer`'s backlog, so the next row for it reads it again.
    pub(crate) fn forget(&self, peer: &str) {
        self.lock().remove(peer);
    }
}

/// A conversation whose invite a remote member's relay should get again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reinvite {
    pub conversation_id: Uuid,
    pub principal_id: String,
}

/// The `(conversation, principal)` pairs whose invite was sent again in the last
/// `REINVITE_INTERVAL`, so a relay refusing every item of a conversation gets it once a day.
#[derive(Default)]
pub struct ReinviteGuard {
    sent: Mutex<HashMap<(Uuid, String), Instant>>,
}

impl ReinviteGuard {
    /// Whether the pair's invite may be sent again now; records it when it may.
    pub fn admit(&self, conversation_id: Uuid, principal_id: &str) -> bool {
        self.admit_at(conversation_id, principal_id, Instant::now())
    }

    /// [`Self::admit`] at `now`. A refused pair keeps the time it was last admitted, so a
    /// relay refusing every day still gets the invite once a day.
    fn admit_at(&self, conversation_id: Uuid, principal_id: &str, now: Instant) -> bool {
        let mut sent = match self.sent.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        sent.retain(|_, at| now.saturating_duration_since(*at) < REINVITE_INTERVAL);
        let key = (conversation_id, principal_id.to_string());
        if sent.contains_key(&key) {
            return false;
        }
        sent.insert(key, now);
        true
    }
}

/// The conversations of a delivery's `not_subscribed` refusals, with the principals refused:
/// each refused item naming a conversation, for each principal the refusal names, or every
/// recipient of the item when it names none.
pub(crate) fn not_subscribed(
    refused: &[RefusedItem],
    items: &[DeliverItem],
) -> Vec<(Uuid, String)> {
    let mut found = Vec::new();
    for refusal in refused.iter().filter(|r| r.reason == "not_subscribed") {
        let Some(item) = items.iter().find(|i| i.id == refusal.id) else {
            continue;
        };
        let statement = SignedStatement {
            payload: item.envelope.payload.clone(),
            signature: item.envelope.signature.clone(),
        };
        let Some(conversation_id) = read_host_event(&statement)
            .ok()
            .and_then(|e| e.event.get("conversation_id")?.as_str().map(str::to_string))
            .and_then(|id| Uuid::parse_str(&id).ok())
        else {
            continue;
        };
        match &refusal.principal_id {
            Some(principal) => found.push((conversation_id, principal.clone())),
            None => found.extend(item.recipients.iter().map(|p| (conversation_id, p.clone()))),
        }
    }
    found
}

/// The non-lossy sink the observers send every event to; the writer reads it.
pub struct OutboxSink {
    sender: mpsc::UnboundedSender<WorkspaceEvent>,
}

impl OutboxSink {
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<WorkspaceEvent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }
}

impl Broadcaster for OutboxSink {
    fn broadcast(&self, event: WorkspaceEvent) {
        let _ = self.sender.send(event);
    }
}

/// Turns workspace events into signed envelopes and outbox rows.
pub struct HostEventWriter {
    repos: HostRepos,
    db: Option<DatabaseConnection>,
    identity: SharedIdentity,
    direct_space_id: ServerId,
    refusals: Arc<RefusalCache>,
    backlogs: Arc<PeerBacklogs>,
    wake: Arc<Notify>,
    /// Files direct-space events, and workspace DMs, for local members through the request
    /// inbox.
    relay: Option<Arc<dyn RelayService>>,
    /// The operator's DM settings: direct-space events go only to relays they admit.
    dm_policy: DmOperatorPolicy,
    /// The server's default workspace, whose groups go through the request inbox like its DMs;
    /// `None` where none is wired.
    default_workspace: Option<Arc<dyn DefaultWorkspaceService>>,
}

/// Who a `DmInvite` is for.
enum Invitee {
    Remote(RemoteMember),
    /// A local member, by principal id.
    Local(String),
}

impl HostEventWriter {
    pub fn new(
        repos: HostRepos,
        db: Option<DatabaseConnection>,
        identity: SharedIdentity,
        direct_space_id: ServerId,
        refusals: Arc<RefusalCache>,
        backlogs: Arc<PeerBacklogs>,
        wake: Arc<Notify>,
    ) -> Self {
        Self {
            repos,
            db,
            identity,
            direct_space_id,
            refusals,
            backlogs,
            wake,
            relay: None,
            dm_policy: DmOperatorPolicy::default(),
            default_workspace: None,
        }
    }

    /// Let the groups of the server's default workspace go through the request inbox like its
    /// DMs.
    pub fn with_default_workspace(
        mut self,
        default_workspace: Arc<dyn DefaultWorkspaceService>,
    ) -> Self {
        self.default_workspace = Some(default_workspace);
        self
    }

    /// Whether `event` of `workspace_id` goes through the request inbox as a workspace's DM: it
    /// is about a DM, or about a group of the default workspace. A member added to a
    /// conversation is announced without its type, which is then read.
    async fn is_workspace_dm(
        &self,
        workspace_id: ServerId,
        event: &WorkspaceEvent,
    ) -> ServiceResult<bool> {
        let conversation_type = match (event.conversation_type(), event) {
            (Some(conversation_type), _) => conversation_type.to_string(),
            (
                None,
                WorkspaceEvent::MemberAddedToConversation {
                    conversation_id, ..
                },
            ) => {
                if !self.is_default_workspace(workspace_id).await? {
                    return Ok(false);
                }
                match self
                    .repos
                    .conversation_repo
                    .get_by_id(*conversation_id)
                    .await?
                {
                    Some(conversation) => conversation.conversation_type.as_str().to_string(),
                    None => return Ok(false),
                }
            }
            _ => return Ok(false),
        };
        if conversation_type == ConversationType::Direct.as_str() {
            return Ok(true);
        }
        if conversation_type != ConversationType::Group.as_str() {
            return Ok(false);
        }
        self.is_default_workspace(workspace_id).await
    }

    /// Whether `workspace_id` is the server's default workspace now.
    async fn is_default_workspace(&self, workspace_id: ServerId) -> ServiceResult<bool> {
        match &self.default_workspace {
            Some(default_workspace) => Ok(default_workspace.current().await? == Some(workspace_id)),
            None => Ok(false),
        }
    }

    /// Send direct-space events only to relays the operator's outbound DM setting admits.
    pub fn with_dm_policy(mut self, dm_policy: DmOperatorPolicy) -> Self {
        self.dm_policy = dm_policy;
        self
    }

    /// Let local members of the direct space, and of a workspace's DMs, get invites and held
    /// events through the request inbox.
    pub fn with_relay(mut self, relay: Arc<dyn RelayService>) -> Self {
        self.relay = Some(relay);
        self
    }

    /// Write events, and the invites the sender asks to send again, until the event channel
    /// closes or shutdown is signalled.
    pub async fn run(
        self,
        mut events: mpsc::UnboundedReceiver<WorkspaceEvent>,
        mut reinvites: mpsc::UnboundedReceiver<Reinvite>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Some(event) => {
                        if let Err(e) = self.write(&event).await {
                            error!(error = %e, "could not write a host event to the outbox");
                        }
                    }
                    None => break,
                },
                Some(reinvite) = reinvites.recv() => {
                    if let Err(e) = self
                        .reinvite(reinvite.conversation_id, &reinvite.principal_id)
                        .await
                    {
                        error!(error = %e, "could not send a conversation's invite again");
                    }
                },
                _ = shutdown.changed() => break,
            }
        }
    }

    /// Write one event's envelopes. Returns how many outbox rows were added.
    pub async fn write(&self, event: &WorkspaceEvent) -> ServiceResult<usize> {
        let Ok(workspace_id) = Uuid::parse_str(&event.workspace_id()) else {
            return Ok(0);
        };
        let workspace_id = ServerId::from(workspace_id);
        if self.relay.is_some()
            && let WorkspaceEvent::MemberRemovedFromConversation {
                conversation_id,
                user_id,
                ..
            } = event
        {
            self.forget_accepted(*user_id, (*conversation_id).into())
                .await?;
        }
        let in_direct_space = workspace_id == self.direct_space_id;
        let direct = in_direct_space && self.relay.is_some();
        // A workspace's DMs, and the default workspace's groups, go through its local members'
        // request inbox too.
        let workspace_dm = !in_direct_space
            && self.relay.is_some()
            && self.is_workspace_dm(workspace_id, event).await?;
        let (members, delegated) = if in_direct_space {
            (
                self.repos.host_repo.remote_members(workspace_id).await?,
                None,
            )
        } else {
            self.delegated(workspace_id, event).await?
        };
        if members.is_empty() && !direct && !workspace_dm {
            return Ok(0);
        }
        let event_json = serde_json::to_value(event).map_err(ServiceError::internal)?;
        let mut rows = 0;
        if in_direct_space || workspace_dm {
            for (invite, invitee) in self.dm_invites(event, &members).await? {
                let planned = match invitee {
                    // A workspace's remote members get its events through their delegations.
                    Invitee::Remote(_) if workspace_dm => continue,
                    Invitee::Remote(member) => Planned {
                        stream: user_stream(&member),
                        recipients: vec![member],
                        device_id: None,
                        local: Vec::new(),
                    },
                    Invitee::Local(principal_id) if direct || workspace_dm => Planned {
                        stream: format!("u:{principal_id}"),
                        recipients: Vec::new(),
                        device_id: None,
                        local: vec![principal_id],
                    },
                    Invitee::Local(_) => continue,
                };
                rows += self
                    .append(workspace_id, &planned, &invite.to_string(), None)
                    .await?;
            }
        }
        let mut plans = plan(event, &event_json, &members);
        if (direct || workspace_dm)
            && let Route::Conversation(conversation_id) = route(event, &event_json)
            && let Some(relay) = &self.relay
        {
            // A local member whose request lapsed gets the invite again first, so the event that
            // follows files their request anew.
            for principal_id in relay.uninvited_holders(conversation_id).await? {
                rows += self
                    .invite_again_locally(workspace_id, conversation_id, &principal_id)
                    .await?;
            }
            let holders = relay.pending_holders(conversation_id).await?;
            if !holders.is_empty() {
                let stream = format!("c:{conversation_id}");
                match plans.iter_mut().find(|p| p.stream == stream) {
                    Some(planned) => planned.local = holders,
                    None => plans.push(Planned {
                        stream,
                        recipients: Vec::new(),
                        device_id: None,
                        local: holders,
                    }),
                }
            }
        }
        let text = event_json.to_string();
        for planned in plans {
            let private_text = if planned.stream.starts_with("u:") && planned.recipients.len() == 1
            {
                Some(
                    event
                        .json_for_user(&planned.recipients[0].user_id.to_string())
                        .map_err(ServiceError::internal)?
                        .to_string(),
                )
            } else {
                None
            };
            let text = private_text.as_deref().unwrap_or(&text);
            rows += self
                .append(workspace_id, &planned, text, delegated.as_ref())
                .await?;
        }
        if rows > 0 {
            self.wake.notify_one();
        }
        if let (Some(_), WorkspaceEvent::MemberLeft { user_id, .. }) = (&delegated, event) {
            let ended = self
                .repos
                .delegation_repo
                .end_for_user(workspace_id, *user_id, None)
                .await?;
            if ended > 0 {
                info!(user_id = %user_id, workspace_id = %workspace_id, ended, "ended a departed member's delegations");
            }
        }
        Ok(rows)
    }

    /// The members of a workspace whose relays hold an active delegation, and those relays by
    /// user. A member who just left still gets their `MemberLeft`, on their own stream, and a
    /// person of another server who asked to join gets the decision through the relay that
    /// filed the request.
    async fn delegated(
        &self,
        workspace_id: ServerId,
        event: &WorkspaceEvent,
    ) -> ServiceResult<(Vec<RemoteMember>, Option<HashMap<UserId, Vec<String>>>)> {
        let now = Utc::now();
        let mut members = self
            .repos
            .delegation_repo
            .delegated_members(workspace_id, now)
            .await?;
        let mut relays: HashMap<UserId, Vec<String>> = HashMap::new();
        for (user_id, relay) in self
            .repos
            .delegation_repo
            .delegated_relays(workspace_id, now)
            .await?
        {
            relays.entry(user_id).or_default().push(relay);
        }
        if let WorkspaceEvent::MemberLeft { user_id, .. } = event
            && relays.contains_key(user_id)
            && !members.iter().any(|m| m.user_id == *user_id)
            && let Some(principal_id) = self
                .repos
                .user_repo
                .get_by_id(*user_id)
                .await?
                .and_then(|u| u.principal_id)
        {
            members.push(RemoteMember {
                user_id: *user_id,
                principal_id,
                role: rim_core::server_member::model::ServerMemberRole::Guest,
                home_relay: None,
                card_payload: None,
            });
        }
        if let WorkspaceEvent::JoinRequestDecided {
            request_id,
            user_id,
            ..
        } = event
            && !relays.contains_key(user_id)
            && let Some((relay, principal_id)) =
                self.repos.host_repo.join_request_relay(*request_id).await?
        {
            relays.insert(*user_id, vec![relay]);
            members.push(RemoteMember {
                user_id: *user_id,
                principal_id,
                role: rim_core::server_member::model::ServerMemberRole::Guest,
                home_relay: None,
                card_payload: None,
            });
        }
        Ok((members, Some(relays)))
    }

    /// `DmInvite` events for the members a conversation of the direct space, or a workspace DM,
    /// was created with or added them to, but its creator: remote members, and local ones with a
    /// principal id. Creating a DM records them as the creator's contacts.
    async fn dm_invites(
        &self,
        event: &WorkspaceEvent,
        members: &[RemoteMember],
    ) -> ServiceResult<Vec<(Value, Invitee)>> {
        let (conversation_id, added, added_by): (_, Vec<UserId>, _) = match event {
            WorkspaceEvent::ConversationCreated {
                conversation_id,
                member_user_ids,
                ..
            } => (*conversation_id, member_user_ids.clone(), None),
            WorkspaceEvent::MemberAddedToConversation {
                conversation_id,
                user_id,
                added_by,
                ..
            } => (*conversation_id, vec![*user_id], *added_by),
            _ => return Ok(Vec::new()),
        };
        if added.is_empty() {
            return Ok(Vec::new());
        }
        let Some(conversation) = self
            .repos
            .conversation_repo
            .get_by_id(conversation_id)
            .await?
        else {
            return Ok(Vec::new());
        };
        let Some(creator) = conversation.created_by else {
            return Ok(Vec::new());
        };
        // An invite comes from whoever brought the member in, so their blocks and DM settings
        // judge that person: the adder, else the creator.
        let inviter = added_by.unwrap_or(creator);
        if added_by.is_some() {
            for user_id in added.iter().filter(|id| **id != inviter) {
                self.repos
                    .conversation_member_repo
                    .set_invited_by(conversation_id, *user_id, inviter)
                    .await?;
            }
        }
        let inviter_user = self.repos.user_repo.get_by_id(inviter).await?;
        let Some(from_principal_id) = inviter_user.and_then(|u| u.principal_id) else {
            return Ok(Vec::new());
        };
        let member_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(conversation_id)
            .await?;
        let users = self.repos.user_repo.get_by_ids(&member_ids).await?;
        let invitees: Vec<Invitee> = added
            .iter()
            .filter(|id| **id != inviter)
            .filter_map(|id| match members.iter().find(|m| m.user_id == *id) {
                Some(member) => Some(Invitee::Remote(member.clone())),
                None => users
                    .iter()
                    .find(|u| u.id == *id)
                    .and_then(|u| u.principal_id.clone())
                    .map(Invitee::Local),
            })
            .collect();
        if matches!(event, WorkspaceEvent::ConversationCreated { .. })
            && conversation.conversation_type == ConversationType::Direct
        {
            self.record_contacts(creator, &invitees).await?;
        }
        let Some(invite) = self
            .invite_event(&conversation, inviter, &from_principal_id, &users)
            .await?
        else {
            return Ok(Vec::new());
        };
        Ok(invitees.into_iter().map(|m| (invite.clone(), m)).collect())
    }

    /// The `DmInvite` of `conversation` from `inviter`, its creator or the member who added
    /// someone, whose principal id is `from_principal_id`, naming the principal ids of `users`,
    /// its members. `None` when the inviter has no profile card.
    async fn invite_event(
        &self,
        conversation: &Conversation,
        inviter: UserId,
        from_principal_id: &str,
        users: &[User],
    ) -> ServiceResult<Option<Value>> {
        let Some(card) = self
            .repos
            .key_identity_repo
            .get_profile_card(inviter)
            .await?
        else {
            warn!(conversation_id = %conversation.id, "no DM invite: the inviter has no profile card");
            return Ok(None);
        };
        let member_principal_ids: Vec<String> = users
            .iter()
            .filter_map(|u| u.principal_id.clone())
            .collect();
        Ok(Some(json!({
            "type": "DmInvite",
            "workspace_id": conversation.server_id,
            "conversation_id": conversation.id,
            "from_principal_id": from_principal_id,
            "from_card": SignedJson {
                payload: card.payload,
                signature: card.signature,
            },
            "member_principal_ids": member_principal_ids,
        })))
    }

    /// Send the `DmInvite` of a conversation of the direct space again to a remote member whose
    /// relay refused an item of it (`not_subscribed`), so their relay judges it afresh like any
    /// invite. It comes from whoever the first invite came from: the member who added them
    /// while still a member, else the creator. Nothing is sent for a conversation elsewhere or
    /// gone, or for a principal who is not a remote member of it. Returns how many outbox rows
    /// were added.
    pub async fn reinvite(
        &self,
        conversation_id: Uuid,
        principal_id: &str,
    ) -> ServiceResult<usize> {
        let Some(conversation) = self
            .repos
            .conversation_repo
            .get_by_id(conversation_id.into())
            .await?
        else {
            return Ok(0);
        };
        if conversation.server_id != self.direct_space_id {
            return Ok(0);
        }
        let Some((user_id, UserKind::Remote)) =
            self.repos.host_repo.find_user(principal_id).await?
        else {
            return Ok(0);
        };
        if self
            .repos
            .conversation_member_repo
            .membership(conversation.id, user_id)
            .await?
            .is_none()
        {
            return Ok(0);
        }
        let members = self
            .repos
            .host_repo
            .remote_members(self.direct_space_id)
            .await?;
        let Some(member) = members.into_iter().find(|m| m.user_id == user_id) else {
            return Ok(0);
        };
        let Some((inviter, from_principal_id)) = self.reinviter(&conversation, user_id).await?
        else {
            return Ok(0);
        };
        let member_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(conversation.id)
            .await?;
        let users = self.repos.user_repo.get_by_ids(&member_ids).await?;
        let Some(invite) = self
            .invite_event(&conversation, inviter, &from_principal_id, &users)
            .await?
        else {
            return Ok(0);
        };
        let planned = Planned {
            stream: user_stream(&member),
            recipients: vec![member],
            device_id: None,
            local: Vec::new(),
        };
        let rows = self
            .append(self.direct_space_id, &planned, &invite.to_string(), None)
            .await?;
        if rows > 0 {
            info!(conversation_id = %conversation_id, "sent a conversation's invite again to a member whose relay no longer held it");
            self.wake.notify_one();
        }
        Ok(rows)
    }

    /// Send the `DmInvite` of a conversation of `workspace_id` again to a local member whose
    /// cleared request for it holds nothing, so their request inbox files it anew with the event
    /// that follows. It comes from whoever the first invite came from (see [`Self::reinviter`]).
    /// Nothing is sent for a conversation gone or a principal who is not a local user. Returns
    /// how many outbox rows were added.
    async fn invite_again_locally(
        &self,
        workspace_id: ServerId,
        conversation_id: Uuid,
        principal_id: &str,
    ) -> ServiceResult<usize> {
        let Some(conversation) = self
            .repos
            .conversation_repo
            .get_by_id(conversation_id.into())
            .await?
        else {
            return Ok(0);
        };
        let Some((user_id, UserKind::Local)) = self.repos.host_repo.find_user(principal_id).await?
        else {
            return Ok(0);
        };
        let Some((inviter, from_principal_id)) = self.reinviter(&conversation, user_id).await?
        else {
            return Ok(0);
        };
        let member_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(conversation.id)
            .await?;
        let users = self.repos.user_repo.get_by_ids(&member_ids).await?;
        let Some(invite) = self
            .invite_event(&conversation, inviter, &from_principal_id, &users)
            .await?
        else {
            return Ok(0);
        };
        let planned = Planned {
            stream: format!("u:{principal_id}"),
            recipients: Vec::new(),
            device_id: None,
            local: vec![principal_id.to_string()],
        };
        let rows = self
            .append(workspace_id, &planned, &invite.to_string(), None)
            .await?;
        info!(conversation_id = %conversation_id, "sent a conversation's invite again to a local member whose request lapsed");
        Ok(rows)
    }

    /// Who a conversation's invite to `user_id` is sent again from, with their principal id:
    /// the member recorded as having added them while still a member with a principal id, else
    /// the conversation's creator. `None` when neither has a principal id.
    async fn reinviter(
        &self,
        conversation: &Conversation,
        user_id: UserId,
    ) -> ServiceResult<Option<(UserId, String)>> {
        let members = &self.repos.conversation_member_repo;
        let adder = match members.invited_by(conversation.id, user_id).await? {
            Some(adder) if members.membership(conversation.id, adder).await?.is_some() => {
                Some(adder)
            }
            _ => None,
        };
        for inviter in adder.into_iter().chain(conversation.created_by) {
            if let Some(principal_id) = self
                .repos
                .user_repo
                .get_by_id(inviter)
                .await?
                .and_then(|u| u.principal_id)
            {
                return Ok(Some((inviter, principal_id)));
            }
        }
        Ok(None)
    }

    /// Delete the accepted row of a local member removed from a conversation of this server, so
    /// the request inbox keeps no row for a conversation they left.
    async fn forget_accepted(&self, user_id: UserId, conversation_id: Uuid) -> ServiceResult<()> {
        let own_id = self.identity.current().server_id().to_string();
        let deleted = self
            .repos
            .relay_repo
            .delete_accepted_request(user_id, &own_id, conversation_id)
            .await?;
        if deleted > 0 {
            debug!(user_id = %user_id, conversation_id = %conversation_id, "deleted a removed member's accepted conversation");
        }
        Ok(())
    }

    /// Record the other party of a DM a local user created as their contact, so a DM back from
    /// them is no request. An existing contact keeps its relay URLs, and none is added once
    /// the creator holds the most contacts a user may.
    async fn record_contacts(&self, creator: UserId, invitees: &[Invitee]) -> ServiceResult<()> {
        if self.repos.host_repo.user_kind(creator).await? != Some(UserKind::Local) {
            return Ok(());
        }
        for invitee in invitees {
            let (principal_id, card) = match invitee {
                Invitee::Remote(member) => (&member.principal_id, member.card_payload.clone()),
                Invitee::Local(principal_id) => {
                    let card = match self.repos.host_repo.find_user(principal_id).await? {
                        Some((user_id, _)) => self
                            .repos
                            .key_identity_repo
                            .get_profile_card(user_id)
                            .await?
                            .map(|c| c.payload),
                        None => None,
                    };
                    (principal_id, card)
                }
            };
            let relay_repo = &self.repos.relay_repo;
            if relay_repo.is_contact(creator, principal_id).await?
                || relay_repo.count_contacts(creator).await? >= MAX_CONTACTS_PER_USER
            {
                continue;
            }
            let relays = card.as_deref().map(contact_relays).unwrap_or_default();
            self.repos
                .relay_repo
                .upsert_contact(creator, principal_id, relays)
                .await?;
        }
        Ok(())
    }

    /// Sign one envelope and add an outbox row for each relay its recipients use, in one
    /// transaction that also takes the stream's `seq`. A workspace's relays are those holding
    /// a delegation (`delegated`); the direct space's are those each card lists. A relay that is
    /// dead or at its backlog caps gets no row, but the event still takes its `seq`, so that
    /// relay's clients see the gap.
    async fn append(
        &self,
        workspace_id: ServerId,
        planned: &Planned,
        event_json: &str,
        delegated: Option<&HashMap<UserId, Vec<String>>>,
    ) -> ServiceResult<usize> {
        let identity = self.identity.current();
        let own_id = identity.server_id().to_string();
        let mut by_relay: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for member in &planned.recipients {
            let relays = match delegated {
                Some(delegated) => delegated
                    .get(&member.user_id)
                    .map(|relays| relays.iter().filter(|r| **r != own_id).cloned().collect())
                    .unwrap_or_default(),
                None => {
                    DefaultHostService::relays_of(
                        self.repos.host_repo.as_ref(),
                        &own_id,
                        member.card_payload.as_deref(),
                        member.home_relay.as_deref(),
                    )
                    .await?
                }
            };
            for relay in relays {
                if self.refusals.is_refused(&relay, &member.principal_id) {
                    continue;
                }
                if workspace_id == self.direct_space_id
                    && outbound(&self.dm_policy, &own_id, &relay) == DmPolicy::Refuse
                {
                    continue;
                }
                by_relay
                    .entry(relay)
                    .or_default()
                    .push(member.principal_id.clone());
            }
        }
        let addressed = !by_relay.is_empty();
        let mut admitted = BTreeMap::new();
        for (relay, recipients) in by_relay {
            if self
                .backlogs
                .admit(self.repos.host_repo.as_ref(), &relay)
                .await?
            {
                admitted.insert(relay, recipients);
            }
        }
        let by_relay = admitted;
        if !addressed && planned.local.is_empty() {
            return Ok(0);
        }
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_host(),
            None => self.repos.clone(),
        };
        let seq = if planned.stream == "e" {
            None
        } else {
            let seq = repos
                .host_repo
                .next_stream_seq(workspace_id, &planned.stream)
                .await?;
            Some(u64::try_from(seq).map_err(ServiceError::internal)?)
        };
        let workspace = workspace_id.to_string();
        let issued_at = rim_s2s::timestamp(Utc::now());
        let envelope = sign_host_event(
            &identity.signer,
            &HostEventFields {
                host: &own_id,
                key_seq: identity.state.seq,
                workspace_id: &workspace,
                stream: &planned.stream,
                seq,
                event_json,
                issued_at: &issued_at,
            },
        );
        if envelope.payload.len() > MAX_EVENT_BYTES {
            warn!(stream = %planned.stream, "a host event is over 512 KiB and is not sent");
            return Ok(0);
        }
        let mut rows = 0;
        for (relay, recipients) in &by_relay {
            let item = json!({
                "envelope": { "payload": envelope.payload, "signature": envelope.signature },
                "recipients": recipients,
                "deviceId": planned.device_id,
            })
            .to_string();
            let size = i32::try_from(item.len()).map_err(ServiceError::internal)?;
            repos.host_repo.insert_outbox(relay, &item, size).await?;
            self.backlogs.queued(relay, i64::from(size));
            rows += 1;
        }
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        if !planned.local.is_empty()
            && let Some(relay) = &self.relay
        {
            relay
                .file_local(
                    SignedJson {
                        payload: envelope.payload.clone(),
                        signature: envelope.signature.clone(),
                    },
                    planned.local.clone(),
                )
                .await?;
        }
        debug!(stream = %planned.stream, ?seq, relays = rows, "queued a host event");
        Ok(rows)
    }
}

/// Delivers each relay's outbox rows in id order. A relay whose last delivery failed is tried
/// again when its backoff ends even with nothing left to send, with an empty delivery, so the
/// first success ends its run of failures.
pub struct OutboxSender {
    repos: HostRepos,
    client: Arc<dyn S2sClient>,
    refusals: Arc<RefusalCache>,
    backlogs: Arc<PeerBacklogs>,
    wake: Arc<Notify>,
    /// Where a conversation's invite is asked for again when a relay refuses its items
    /// `not_subscribed`; `None` asks nothing.
    reinvites: Option<mpsc::UnboundedSender<Reinvite>>,
    reinvited: ReinviteGuard,
}

impl OutboxSender {
    pub fn new(
        repos: HostRepos,
        client: Arc<dyn S2sClient>,
        refusals: Arc<RefusalCache>,
        backlogs: Arc<PeerBacklogs>,
        wake: Arc<Notify>,
    ) -> Self {
        Self {
            repos,
            client,
            refusals,
            backlogs,
            wake,
            reinvites: None,
            reinvited: ReinviteGuard::default(),
        }
    }

    /// Ask the writer on `reinvites` to send a conversation's invite again, at most once a day
    /// per conversation and principal, when a relay refuses an item of it `not_subscribed`.
    pub fn with_reinvites(mut self, reinvites: mpsc::UnboundedSender<Reinvite>) -> Self {
        self.reinvites = Some(reinvites);
        self
    }

    /// Deliver whenever the writer adds rows and at least every second, until shutdown.
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tick.tick() => {}
                _ = shutdown.changed() => break,
            }
            if let Err(e) = self.send_due().await {
                error!(error = %e, "outbox delivery pass failed");
            }
        }
    }

    /// One pass: every relay whose next attempt is due gets its next batch.
    pub async fn send_due(&self) -> ServiceResult<()> {
        let peers = self.repos.host_repo.outbox_peers_due(Utc::now()).await?;
        let sends = peers.iter().map(|peer| self.send_peer(peer));
        for result in futures_util::future::join_all(sends).await {
            if let Err(e) = result {
                error!(error = %e, "outbox delivery to a relay failed");
            }
        }
        Ok(())
    }

    /// Send a relay its next batch, or an empty one while its deliveries fail. Returns whether
    /// it acknowledged it. A dead relay waits the longest backoff between attempts.
    pub async fn send_peer(&self, peer: &str) -> ServiceResult<bool> {
        let delivery = self.repos.host_repo.delivery(peer).await?;
        let rows = self
            .repos
            .host_repo
            .outbox_after(
                peer,
                delivery.delivered_through,
                MAX_DELIVER_ITEMS as i64,
                MAX_DELIVER_BYTES,
            )
            .await?;
        let last = match rows.last() {
            Some(row) => row.id,
            None if delivery.failures > 0 => delivery.delivered_through,
            None => return Ok(true),
        };
        let items = rows
            .iter()
            .map(|row| deliver_item(row.id, &row.item))
            .collect::<Result<Vec<_>, _>>()
            .map_err(ServiceError::internal)?;
        let principals: HashMap<String, Vec<String>> = items
            .iter()
            .map(|item| (item.id.clone(), item.recipients.clone()))
            .collect();
        let request = DeliverRequest { items };
        let body = serde_json::to_vec(&request).map_err(ServiceError::internal)?;
        let response = self
            .client
            .send(S2sRequest {
                peer_server_id: peer.to_string(),
                method: S2sMethod::Post,
                path: "/deliver".to_string(),
                body,
                content_type: None,
                headers: Vec::new(),
            })
            .await;
        let answer = match response {
            Ok(r) if r.status == 200 => serde_json::from_slice::<DeliverResponse>(&r.body).ok(),
            Ok(r) => {
                warn!(peer_server_id = %peer, status = r.status, "a relay refused a delivery");
                None
            }
            Err(e) => {
                warn!(peer_server_id = %peer, error = %e, "delivery to a relay failed");
                None
            }
        };
        let Some(answer) = answer else {
            let now = Utc::now();
            let failures = delivery.failures.saturating_add(1);
            let backoff = if self.backlogs.is_dead(delivery.failing_since, now) {
                MAX_BACKOFF_SECS
            } else {
                1_i64
                    .checked_shl(failures.clamp(1, 16) as u32 - 1)
                    .unwrap_or(MAX_BACKOFF_SECS)
                    .min(MAX_BACKOFF_SECS)
            };
            self.repos
                .host_repo
                .record_failure(peer, failures, now + Duration::seconds(backoff))
                .await?;
            return Ok(false);
        };
        for refusal in &answer.refused {
            let refused = match &refusal.principal_id {
                Some(p) => vec![p.clone()],
                None => principals.get(&refusal.id).cloned().unwrap_or_default(),
            };
            // Only a principal the relay does not hold is skipped as a whole; other refusals are
            // about one conversation.
            if refusal.reason == "unknown_principal" {
                for principal in refused {
                    self.refusals.refuse(peer, &principal);
                }
            }
        }
        // A relay that holds no row for a conversation any longer (expired after a long
        // silence) gets its invite again, and judges it afresh.
        if let Some(reinvites) = &self.reinvites {
            for (conversation_id, principal_id) in not_subscribed(&answer.refused, &request.items) {
                if self.reinvited.admit(conversation_id, &principal_id) {
                    let _ = reinvites.send(Reinvite {
                        conversation_id,
                        principal_id,
                    });
                }
            }
        }
        let acked = answer
            .acked_through
            .parse::<i64>()
            .unwrap_or(delivery.delivered_through)
            .min(last);
        self.repos.host_repo.record_delivered(peer, acked).await?;
        if delivery.failures > 0 {
            self.backlogs.forget(peer);
            info!(peer_server_id = %peer, failures = delivery.failures, "a relay takes deliveries again");
        }
        info!(peer_server_id = %peer, acked_through = acked, refused = answer.refused.len(), "delivered host events");
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rim_core::auth::repository::MockSessionRepository;
    use rim_core::conversation::model::ConversationId;
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::dm_policy::repository::MockDmPolicyRepository;
    use rim_core::e2ee::repository::MockE2eeRepository;
    use rim_core::host::model::{OutboxBacklog, OutboxRow, PeerDelivery};
    use rim_core::host::repository::{MockDelegationRepository, MockHostRepository};
    use rim_core::key_identity::repository::MockKeyIdentityRepository;
    use rim_core::relay::repository::{MockRelayAbuseRepository, MockRelayRepository};
    use rim_core::server_member::model::ServerMemberRole;
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::repository::MockUserRepository;
    use rim_services::s2s::dto::{S2sClientError, S2sResponse};
    use rim_services::s2s::service::MockS2sClient;

    use super::*;
    use crate::host::identity_tests::server_identity;
    use crate::metrics::test_support::{block_on, recorded};

    fn member(role: ServerMemberRole) -> RemoteMember {
        RemoteMember {
            user_id: UserId::new(),
            principal_id: format!("rimp1_{}", Uuid::now_v7()),
            role,
            home_relay: None,
            card_payload: None,
        }
    }

    fn planned(event: &WorkspaceEvent, members: &[RemoteMember]) -> Vec<Planned> {
        plan(event, &serde_json::to_value(event).expect("json"), members)
    }

    #[test]
    fn conversation_events_go_on_the_conversation_stream_to_its_members() {
        let (a, b) = (
            member(ServerMemberRole::Guest),
            member(ServerMemberRole::Guest),
        );
        let conversation_id = ConversationId::new();
        let event = WorkspaceEvent::MlsCommitAccepted {
            workspace_id: ServerId::new(),
            conversation_id,
            incarnation: 1,
            epoch: 2,
            member_user_ids: vec![a.user_id],
        };
        let plans = planned(&event, &[a.clone(), b]);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].stream, format!("c:{conversation_id}"));
        assert_eq!(plans[0].recipients, vec![a]);
    }

    #[test]
    fn user_events_go_one_envelope_per_recipient_with_the_device() {
        let a = member(ServerMemberRole::Guest);
        let device_id = Uuid::now_v7();
        let event = WorkspaceEvent::MlsWelcomeAvailable {
            workspace_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            user_id: a.user_id,
            device_id,
            member_user_ids: vec![a.user_id],
        };
        let plans = planned(&event, &[a.clone(), member(ServerMemberRole::Guest)]);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].stream, format!("u:{}", a.principal_id));
        assert_eq!(plans[0].device_id, Some(device_id));
    }

    #[test]
    fn directory_events_reach_members_on_w_and_guests_about_themselves() {
        let (guest, regular) = (
            member(ServerMemberRole::Guest),
            member(ServerMemberRole::Member),
        );
        let event = WorkspaceEvent::MemberJoined {
            workspace_id: ServerId::new(),
            user_id: guest.user_id,
            username: "g".to_string(),
            source: None,
        };
        let plans = planned(&event, &[guest.clone(), regular.clone()]);
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].stream, "w");
        assert_eq!(plans[0].recipients, vec![regular]);
        assert_eq!(plans[1].stream, format!("u:{}", guest.principal_id));
        let other = WorkspaceEvent::MemberJoined {
            workspace_id: ServerId::new(),
            user_id: UserId::new(),
            username: "x".to_string(),
            source: None,
        };
        let plans = planned(&other, &[guest]);
        assert!(plans.is_empty(), "a guest learns nothing about others");
    }

    #[test]
    fn typing_is_ephemeral_and_workspace_events_skip_guests() {
        let (guest, regular) = (
            member(ServerMemberRole::Guest),
            member(ServerMemberRole::Member),
        );
        let typing = WorkspaceEvent::PresenceChanged {
            workspace_id: ServerId::new(),
            user_id: UserId::new(),
            status: "online".to_string(),
        };
        let plans = planned(&typing, std::slice::from_ref(&guest));
        assert_eq!(plans[0].stream, "e");
        let plugin = WorkspaceEvent::WorkspacePluginChanged {
            workspace_id: ServerId::new(),
            plugin_id: "p".to_string(),
            change: rim_services::events::WorkspacePluginChange::Added,
        };
        let plans = planned(&plugin, &[guest, regular.clone()]);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].stream, "w");
        assert_eq!(plans[0].recipients, vec![regular]);
    }

    #[test]
    fn a_rename_goes_on_w_to_members_only() {
        let (guest, regular) = (
            member(ServerMemberRole::Guest),
            member(ServerMemberRole::Member),
        );
        let rename = WorkspaceEvent::WorkspaceUpdated {
            workspace_id: ServerId::new(),
            name: "Platform".to_string(),
        };
        let plans = planned(&rename, &[guest, regular.clone()]);
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].stream, "w");
        assert_eq!(plans[0].recipients, vec![regular]);
    }

    #[test]
    fn removed_members_get_a_private_stream_for_their_reason() {
        let removed = member(ServerMemberRole::Member);
        let other = member(ServerMemberRole::Member);
        let event = WorkspaceEvent::MemberLeft {
            workspace_id: ServerId::new(),
            user_id: removed.user_id,
            cause: Some("ban".to_string()),
            until: None,
            reason: Some("private".to_string()),
        };
        let plans = planned(&event, &[removed.clone(), other.clone()]);
        assert!(
            plans.iter().any(|plan| plan.stream == user_stream(&removed)
                && plan.recipients == vec![removed.clone()])
        );
        assert!(
            plans
                .iter()
                .any(|plan| plan.stream == "w" && plan.recipients == vec![other.clone()])
        );
        assert!(
            serde_json::to_value(&event)
                .unwrap()
                .get("reason")
                .is_none()
        );
    }

    #[test]
    fn delivered_items_carry_their_outbox_id() {
        let item = deliver_item(
            42,
            r#"{"envelope":{"payload":"p","signature":"s"},"recipients":["rimp1_a"],"deviceId":null}"#,
        )
        .expect("item");
        assert_eq!(item.id, "42");
        assert_eq!(item.recipients, vec!["rimp1_a"]);
    }

    #[test]
    fn refusals_are_kept_per_relay_and_principal() {
        let cache = RefusalCache::default();
        cache.refuse("rims1_a", "rimp1_x");
        assert!(cache.is_refused("rims1_a", "rimp1_x"));
        assert!(!cache.is_refused("rims1_b", "rimp1_x"));
    }

    /// A relay's backlog, and the outbox rows and failures the mocked host repository records.
    #[derive(Default)]
    struct Outbox {
        /// The backlogs `outbox_backlog` answers, one per read; the last repeats.
        backlogs: Mutex<Vec<OutboxBacklog>>,
        reads: AtomicUsize,
        inserted: AtomicUsize,
        seqs: AtomicUsize,
        failures: Mutex<Vec<(i32, DateTime<Utc>)>>,
        delivered: Mutex<Vec<i64>>,
    }

    fn backlog(rows: i64, bytes: i64, failing_since: Option<DateTime<Utc>>) -> OutboxBacklog {
        OutboxBacklog {
            rows,
            bytes,
            failing_since,
        }
    }

    fn host_repo(outbox: &Arc<Outbox>) -> MockHostRepository {
        let mut host = MockHostRepository::new();
        let o = Arc::clone(outbox);
        host.expect_outbox_backlog().returning(move |_| {
            let n = o.reads.fetch_add(1, Ordering::SeqCst);
            let backlogs = o.backlogs.lock().expect("lock");
            let answer = backlogs[n.min(backlogs.len() - 1)].clone();
            Box::pin(async move { Ok(answer) })
        });
        let o = Arc::clone(outbox);
        host.expect_next_stream_seq().returning(move |_, _| {
            let seq = o.seqs.fetch_add(1, Ordering::SeqCst) as i64 + 1;
            Box::pin(async move { Ok(seq) })
        });
        let o = Arc::clone(outbox);
        host.expect_insert_outbox().returning(move |_, _, _| {
            let id = o.inserted.fetch_add(1, Ordering::SeqCst) as i64 + 1;
            Box::pin(async move { Ok(id) })
        });
        let o = Arc::clone(outbox);
        host.expect_record_failure()
            .returning(move |_, failures, next_attempt_at| {
                o.failures
                    .lock()
                    .expect("lock")
                    .push((failures, next_attempt_at));
                Box::pin(async { Ok(()) })
            });
        let o = Arc::clone(outbox);
        host.expect_record_delivered().returning(move |_, through| {
            o.delivered.lock().expect("lock").push(through);
            Box::pin(async { Ok(()) })
        });
        host
    }

    fn repos(host: MockHostRepository) -> HostRepos {
        HostRepos {
            host_repo: Arc::new(host),
            relay_repo: Arc::new(MockRelayRepository::new()),
            key_identity_repo: Arc::new(MockKeyIdentityRepository::new()),
            e2ee_repo: Arc::new(MockE2eeRepository::new()),
            user_repo: Arc::new(MockUserRepository::new()),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            server_member_repo: Arc::new(MockServerMemberRepository::new()),
            delegation_repo: Arc::new(MockDelegationRepository::new()),
            session_repo: Arc::new(MockSessionRepository::new()),
            relay_abuse_repo: Arc::new(MockRelayAbuseRepository::new()),
            dm_policy_repo: Arc::new(MockDmPolicyRepository::new()),
        }
    }

    const DEAD: &str = "rims1_dead";
    const DEAD_AFTER: Duration = Duration::hours(24);

    /// Write `events` conversation events whose one recipient's delegation is held by `DEAD`,
    /// with the repository answering `backlogs` one read after another. Returns what the
    /// repository saw and the shared backlogs.
    async fn write_to_relay(
        backlogs: Vec<OutboxBacklog>,
        events: usize,
    ) -> (Arc<Outbox>, Arc<PeerBacklogs>) {
        let outbox = Arc::new(Outbox {
            backlogs: Mutex::new(backlogs),
            ..Outbox::default()
        });
        let peer_backlogs = Arc::new(PeerBacklogs::new(DEAD_AFTER));
        let writer = HostEventWriter::new(
            repos(host_repo(&outbox)),
            None,
            server_identity(),
            ServerId::new(),
            Arc::new(RefusalCache::default()),
            Arc::clone(&peer_backlogs),
            Arc::new(Notify::new()),
        );
        let bob = member(ServerMemberRole::Member);
        let delegated = HashMap::from([(bob.user_id, vec![DEAD.to_string()])]);
        let planned = Planned {
            stream: "c:x".to_string(),
            recipients: vec![bob],
            device_id: None,
            local: Vec::new(),
        };
        for _ in 0..events {
            writer
                .append(
                    ServerId::new(),
                    &planned,
                    r#"{"type":"NewMessage"}"#,
                    Some(&delegated),
                )
                .await
                .expect("append");
        }
        (outbox, peer_backlogs)
    }

    #[test]
    fn an_unreachable_relay_gets_no_rows_past_the_row_cap() {
        let failing = Some(Utc::now() - Duration::hours(1));
        let ((outbox, _), got) = recorded(|| {
            block_on(write_to_relay(
                vec![backlog(MAX_PEER_OUTBOX_ROWS - 1, 0, failing)],
                5,
            ))
        });
        assert_eq!(
            outbox.inserted.load(Ordering::SeqCst),
            1,
            "one row fills the cap"
        );
        assert_eq!(
            outbox.reads.load(Ordering::SeqCst),
            1,
            "rows the writer adds are counted without reading again"
        );
        assert_eq!(
            outbox.seqs.load(Ordering::SeqCst),
            5,
            "every event takes its seq, so the relay's clients see the gap"
        );
        assert_eq!(
            got.counter(crate::metrics::OUTBOX_NOT_QUEUED, &[("reason", "peer_cap")]),
            4
        );
    }

    #[test]
    fn an_unreachable_relay_gets_no_rows_past_the_byte_cap() {
        let ((outbox, _), got) = recorded(|| {
            block_on(write_to_relay(
                vec![backlog(10, MAX_PEER_OUTBOX_BYTES - 1, None)],
                3,
            ))
        });
        assert_eq!(outbox.inserted.load(Ordering::SeqCst), 1);
        assert_eq!(
            got.counter(crate::metrics::OUTBOX_NOT_QUEUED, &[("reason", "peer_cap")]),
            2
        );
    }

    #[tokio::test]
    async fn a_relay_below_the_caps_gets_every_row() {
        let (outbox, _) = write_to_relay(vec![backlog(10, 1_000, None)], 20).await;
        assert_eq!(outbox.inserted.load(Ordering::SeqCst), 20);
    }

    #[test]
    fn a_dead_relay_gets_no_rows_until_its_backlog_is_read_again_after_a_success() {
        let dead = Some(Utc::now() - DEAD_AFTER - Duration::minutes(1));
        let ((outbox, peer_backlogs), got) = recorded(|| {
            block_on(write_to_relay(
                vec![backlog(5, 500, dead), backlog(5, 500, None)],
                3,
            ))
        });
        assert_eq!(outbox.inserted.load(Ordering::SeqCst), 0);
        assert_eq!(outbox.seqs.load(Ordering::SeqCst), 3);
        assert_eq!(
            got.counter(
                crate::metrics::OUTBOX_NOT_QUEUED,
                &[("reason", "dead_peer")]
            ),
            3
        );

        // A success forgets the copy; the next read finds the failures over.
        peer_backlogs.forget(DEAD);
        let host = host_repo(&outbox);
        let admitted = block_on(peer_backlogs.admit(&host, DEAD)).expect("admit");
        assert!(admitted, "queueing resumes after the first success");
    }

    #[test]
    fn a_relay_failing_for_less_than_the_period_is_not_dead() {
        let backlogs = PeerBacklogs::new(Duration::hours(24));
        let now = Utc::now();
        assert!(!backlogs.is_dead(None, now));
        assert!(!backlogs.is_dead(Some(now - Duration::hours(23)), now));
        assert!(backlogs.is_dead(Some(now - Duration::hours(24)), now));
    }

    /// An outbox sender to `DEAD`, whose delivery state is `delivery` and outbox holds `rows`
    /// rows, through a client answering `answer`. Returns what the repository and the client
    /// saw.
    async fn send_to_relay(
        delivery: PeerDelivery,
        rows: i64,
        answer: Result<String, ()>,
        peer_backlogs: Arc<PeerBacklogs>,
    ) -> (Arc<Outbox>, Vec<DeliverRequest>, bool) {
        let outbox = Arc::new(Outbox::default());
        let mut host = host_repo(&outbox);
        host.expect_delivery().returning(move |_| {
            let delivery = delivery.clone();
            Box::pin(async move { Ok(delivery) })
        });
        host.expect_outbox_after().returning(move |_, after, _, _| {
            let rows = (after + 1..=after + rows)
                .map(|id| OutboxRow {
                    id,
                    item: r#"{"envelope":{"payload":"p","signature":"s"},"recipients":["rimp1_a"],"deviceId":null}"#.to_string(),
                    size_bytes: 80,
                })
                .collect();
            Box::pin(async move { Ok(rows) })
        });
        let sent = Arc::new(Mutex::new(Vec::new()));
        let mut client = MockS2sClient::new();
        let s = Arc::clone(&sent);
        client.expect_send().returning(move |request| {
            s.lock()
                .expect("lock")
                .push(serde_json::from_slice::<DeliverRequest>(&request.body).expect("body"));
            let answer = match &answer {
                Ok(body) => Ok(S2sResponse {
                    status: 200,
                    content_type: "application/json".to_string(),
                    body: body.clone().into_bytes(),
                    key_seq: 0,
                    headers: Vec::new(),
                }),
                Err(()) => Err(S2sClientError::Transport("connection refused".to_string())),
            };
            Box::pin(async move { answer })
        });
        let sender = OutboxSender::new(
            repos(host),
            Arc::new(client),
            Arc::new(RefusalCache::default()),
            peer_backlogs,
            Arc::new(Notify::new()),
        );
        let acked = sender.send_peer(DEAD).await.expect("send");
        let sent = sent.lock().expect("lock").clone();
        (outbox, sent, acked)
    }

    fn delivery(failures: i32, failing_since: Option<DateTime<Utc>>) -> PeerDelivery {
        PeerDelivery {
            delivered_through: 7,
            failures,
            next_attempt_at: Utc::now(),
            failing_since,
        }
    }

    #[tokio::test]
    async fn an_idle_relay_that_takes_deliveries_is_not_contacted() {
        let (_, sent, acked) = send_to_relay(
            delivery(0, None),
            0,
            Err(()),
            Arc::new(PeerBacklogs::new(DEAD_AFTER)),
        )
        .await;
        assert!(acked);
        assert!(sent.is_empty());
    }

    #[tokio::test]
    async fn a_failing_relay_with_nothing_left_is_probed_and_its_first_success_ends_the_failures() {
        let dead = Some(Utc::now() - DEAD_AFTER - Duration::hours(1));
        let peer_backlogs = Arc::new(PeerBacklogs::new(DEAD_AFTER));
        let outbox = Arc::new(Outbox {
            backlogs: Mutex::new(vec![backlog(0, 0, dead)]),
            ..Outbox::default()
        });
        assert!(
            !peer_backlogs
                .admit(&host_repo(&outbox), DEAD)
                .await
                .expect("admit"),
            "dead"
        );
        let (outbox, sent, acked) = send_to_relay(
            delivery(40, dead),
            0,
            Ok(r#"{"ackedThrough":"7","refused":[]}"#.to_string()),
            Arc::clone(&peer_backlogs),
        )
        .await;
        assert!(acked);
        assert_eq!(sent.len(), 1, "one probe");
        assert!(sent[0].items.is_empty(), "the probe carries no items");
        assert_eq!(*outbox.delivered.lock().expect("lock"), vec![7]);
        assert!(
            !peer_backlogs.lock().contains_key(DEAD),
            "the writer reads the relay's state again"
        );
    }

    #[tokio::test]
    async fn a_dead_relay_waits_the_longest_backoff_between_attempts() {
        let dead = Some(Utc::now() - DEAD_AFTER - Duration::hours(1));
        let (outbox, sent, acked) = send_to_relay(
            delivery(1, dead),
            3,
            Err(()),
            Arc::new(PeerBacklogs::new(DEAD_AFTER)),
        )
        .await;
        assert!(!acked);
        assert_eq!(sent[0].items.len(), 3, "the probe is the next batch");
        let failures = outbox.failures.lock().expect("lock").clone();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, 2);
        let wait = failures[0].1 - Utc::now();
        assert!(
            wait > Duration::seconds(MAX_BACKOFF_SECS - 5),
            "waited {wait}"
        );

        let failing = Some(Utc::now() - Duration::hours(1));
        let (outbox, _, _) = send_to_relay(
            delivery(1, failing),
            3,
            Err(()),
            Arc::new(PeerBacklogs::new(DEAD_AFTER)),
        )
        .await;
        let wait = outbox.failures.lock().expect("lock")[0].1 - Utc::now();
        assert!(
            wait < Duration::seconds(5),
            "a relay not yet dead backs off as before: {wait}"
        );
    }

    /// An item for `recipients` on `stream` carrying `event`, signed by a test host.
    fn delivered(id: &str, stream: &str, event: Value, recipients: &[&str]) -> DeliverItem {
        let signer = rim_s2s::ServerSigner::from_seed([7; 32]);
        let event = event.to_string();
        let issued_at = rim_s2s::timestamp(Utc::now());
        let envelope = sign_host_event(
            &signer,
            &HostEventFields {
                host: "rims1_host",
                key_seq: 0,
                workspace_id: &Uuid::from_u128(5).to_string(),
                stream,
                seq: Some(1),
                event_json: &event,
                issued_at: &issued_at,
            },
        );
        DeliverItem {
            id: id.to_string(),
            envelope: SignedJson {
                payload: envelope.payload,
                signature: envelope.signature,
            },
            recipients: recipients.iter().map(|r| r.to_string()).collect(),
            device_id: None,
        }
    }

    fn refused(id: &str, reason: &str, principal_id: Option<&str>) -> RefusedItem {
        RefusedItem {
            id: id.to_string(),
            reason: reason.to_string(),
            principal_id: principal_id.map(str::to_string),
        }
    }

    #[test]
    fn not_subscribed_refusals_name_their_conversation_and_principals() {
        let conversation = Uuid::from_u128(9);
        let message = json!({ "type": "NewMessage", "conversation_id": conversation });
        let items = [
            delivered(
                "1",
                &format!("c:{conversation}"),
                message.clone(),
                &["rimp1_a", "rimp1_b"],
            ),
            delivered("2", &format!("c:{conversation}"), message, &["rimp1_c"]),
            delivered("3", "w", json!({ "type": "MemberJoined" }), &["rimp1_d"]),
        ];
        assert_eq!(
            not_subscribed(
                &[
                    refused("1", "not_subscribed", Some("rimp1_b")),
                    refused("2", "not_subscribed", None),
                    refused("3", "not_subscribed", None),
                    refused("1", "unknown_principal", Some("rimp1_a")),
                ],
                &items
            ),
            vec![
                (conversation, "rimp1_b".to_string()),
                (conversation, "rimp1_c".to_string()),
            ],
            "a workspace item names no conversation, another reason asks for nothing"
        );
    }

    /// A relay that was dead while a conversation's invite was written never got it. Back, it
    /// refuses the conversation's items `not_subscribed`, and the sender asks for the invite
    /// again for the member it refused, once however often it refuses.
    #[tokio::test]
    async fn a_relay_that_missed_an_invite_gets_it_again_once_when_it_refuses_the_conversation() {
        let conversation = Uuid::from_u128(9);
        let message = delivered(
            "0",
            &format!("c:{conversation}"),
            json!({ "type": "NewMessage", "conversation_id": conversation }),
            &["rimp1_bob"],
        );
        let row = json!({
            "envelope": { "payload": message.envelope.payload, "signature": message.envelope.signature },
            "recipients": ["rimp1_bob"],
            "deviceId": null,
        })
        .to_string();
        let outbox = Arc::new(Outbox::default());
        let mut host = host_repo(&outbox);
        host.expect_delivery()
            .returning(|_| Box::pin(async { Ok(delivery(0, None)) }));
        host.expect_outbox_after().returning(move |_, after, _, _| {
            let rows = vec![OutboxRow {
                id: after + 1,
                item: row.clone(),
                size_bytes: 80,
            }];
            Box::pin(async move { Ok(rows) })
        });
        let mut client = MockS2sClient::new();
        client.expect_send().returning(|_| {
            Box::pin(async {
                Ok(S2sResponse {
                    status: 200,
                    content_type: "application/json".to_string(),
                    body: br#"{"ackedThrough":"8","refused":[{"id":"8","reason":"not_subscribed","principalId":"rimp1_bob"}]}"#.to_vec(),
                    key_seq: 0,
                    headers: Vec::new(),
                })
            })
        });
        let (reinvites, mut asked) = mpsc::unbounded_channel();
        let sender = OutboxSender::new(
            repos(host),
            Arc::new(client),
            Arc::new(RefusalCache::default()),
            Arc::new(PeerBacklogs::new(DEAD_AFTER)),
            Arc::new(Notify::new()),
        )
        .with_reinvites(reinvites);
        for _ in 0..2 {
            assert!(sender.send_peer(DEAD).await.expect("send"));
        }
        assert_eq!(
            asked.try_recv().expect("asked once"),
            Reinvite {
                conversation_id: conversation,
                principal_id: "rimp1_bob".to_string(),
            }
        );
        assert!(asked.try_recv().is_err(), "not again the same day");
    }

    #[test]
    fn an_invite_is_sent_again_once_a_day_per_conversation_and_principal() {
        let guard = ReinviteGuard::default();
        let (one, two) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert!(guard.admit(one, "rimp1_a"));
        assert!(!guard.admit(one, "rimp1_a"), "the same day");
        assert!(guard.admit(one, "rimp1_b"));
        assert!(guard.admit(two, "rimp1_a"));
    }

    #[test]
    fn a_refused_reinvite_does_not_push_the_next_one_back() {
        let guard = ReinviteGuard::default();
        let one = Uuid::from_u128(1);
        let start = Instant::now();
        let hours = |h: u64| start + std::time::Duration::from_secs(h * 3600);
        assert!(guard.admit_at(one, "rimp1_a", start));
        assert!(
            !guard.admit_at(one, "rimp1_a", hours(23)),
            "refused within the day"
        );
        assert!(
            guard.admit_at(one, "rimp1_a", hours(25)),
            "a day after the last invite, whatever was refused in between"
        );
    }
}
