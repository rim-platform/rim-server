//! Host events for relays: the writer turns each workspace event into signed envelopes on the
//! streams its remote recipients see and appends one outbox row per relay (for a workspace,
//! each relay holding an active delegation of a recipient; for the direct space, the relays a
//! recipient's card lists, those the operator's outbound DM setting admits); it files what local
//! members get through the request inbox: invites and held events of the direct space and of
//! workspace DMs. The sender delivers each relay's rows in order with backoff.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{Duration, Utc};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, watch};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use rim_core::conversation::model::ConversationType;
use rim_core::host::model::{RemoteMember, UserKind};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_infra_db::begin_transaction;
use rim_s2s::SharedIdentity;
use rim_s2s::host::{HostEventFields, sign_host_event};
use rim_services::dm_policy::dto::DmOperatorPolicy;
use rim_services::events::visibility::user_can_see_event_as;
use rim_services::events::{Broadcaster, WorkspaceEvent};
use rim_services::relay::dto::{DeliverItem, DeliverRequest, DeliverResponse, SignedJson};
use rim_services::relay::service::RelayService;
use rim_services::s2s::dto::{S2sMethod, S2sRequest};
use rim_services::s2s::service::S2sClient;
use rim_services::{ServiceError, ServiceResult};

use crate::host::{DefaultHostService, HostRepos};
use crate::relay::policy::{DmPolicy, outbound};
use crate::relay::{MAX_CONTACTS_PER_USER, MAX_DELIVER_BYTES, MAX_DELIVER_ITEMS, contact_relays};
use crate::repos::AppRepos;

/// Largest host event payload.
const MAX_EVENT_BYTES: usize = 512 * 1024;
/// How long a relay's refusal of a principal keeps the writer from addressing them.
const REFUSAL_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
/// Longest wait between delivery attempts.
const MAX_BACKOFF_SECS: i64 = 300;

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
        | WorkspaceEvent::MlsWelcomeAvailable { .. } => Route::User,
        WorkspaceEvent::MemberJoined { .. }
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
                visible.into_iter().partition(|m| m.role.is_guest());
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
    wake: Arc<Notify>,
    /// Files direct-space events, and workspace DMs, for local members through the request
    /// inbox.
    relay: Option<Arc<dyn RelayService>>,
    /// The operator's DM settings: direct-space events go only to relays they admit.
    dm_policy: DmOperatorPolicy,
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
        wake: Arc<Notify>,
    ) -> Self {
        Self {
            repos,
            db,
            identity,
            direct_space_id,
            refusals,
            wake,
            relay: None,
            dm_policy: DmOperatorPolicy::default(),
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

    /// Write events until the channel closes or shutdown is signalled.
    pub async fn run(
        self,
        mut events: mpsc::UnboundedReceiver<WorkspaceEvent>,
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
        let in_direct_space = workspace_id == self.direct_space_id;
        let direct = in_direct_space && self.relay.is_some();
        // A workspace's DMs go through its local members' request inbox too.
        let workspace_dm = !in_direct_space
            && self.relay.is_some()
            && event.conversation_type() == Some(ConversationType::Direct.as_str());
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
            rows += self
                .append(workspace_id, &planned, &text, delegated.as_ref())
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
    /// user. A member who just left still gets their `MemberLeft`, on their own stream.
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
        let (conversation_id, added): (_, Vec<UserId>) = match event {
            WorkspaceEvent::ConversationCreated {
                conversation_id,
                member_user_ids,
                ..
            } => (*conversation_id, member_user_ids.clone()),
            WorkspaceEvent::MemberAddedToConversation {
                conversation_id,
                user_id,
                ..
            } => (*conversation_id, vec![*user_id]),
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
        let creator_user = self.repos.user_repo.get_by_id(creator).await?;
        let Some(from_principal_id) = creator_user.and_then(|u| u.principal_id) else {
            return Ok(Vec::new());
        };
        let member_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(conversation_id)
            .await?;
        let users = self.repos.user_repo.get_by_ids(&member_ids).await?;
        let member_principal_ids: Vec<String> = users
            .iter()
            .filter_map(|u| u.principal_id.clone())
            .collect();
        let invitees: Vec<Invitee> = added
            .iter()
            .filter(|id| **id != creator)
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
        let Some(card) = self
            .repos
            .key_identity_repo
            .get_profile_card(creator)
            .await?
        else {
            warn!(conversation_id = %conversation_id, "no DM invite: the creator has no profile card");
            return Ok(Vec::new());
        };
        Ok(invitees
            .into_iter()
            .map(|m| {
                (
                    json!({
                        "type": "DmInvite",
                        "workspace_id": conversation.server_id,
                        "conversation_id": conversation_id,
                        "from_principal_id": from_principal_id,
                        "from_card": SignedJson {
                            payload: card.payload.clone(),
                            signature: card.signature.clone(),
                        },
                        "member_principal_ids": member_principal_ids,
                    }),
                    m,
                )
            })
            .collect())
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
    /// a delegation (`delegated`); the direct space's are those each card lists.
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
        if by_relay.is_empty() && planned.local.is_empty() {
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

/// Delivers each relay's outbox rows in id order.
pub struct OutboxSender {
    repos: HostRepos,
    client: Arc<dyn S2sClient>,
    refusals: Arc<RefusalCache>,
    wake: Arc<Notify>,
}

impl OutboxSender {
    pub fn new(
        repos: HostRepos,
        client: Arc<dyn S2sClient>,
        refusals: Arc<RefusalCache>,
        wake: Arc<Notify>,
    ) -> Self {
        Self {
            repos,
            client,
            refusals,
            wake,
        }
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

    /// Send a relay its next batch. Returns whether it acknowledged it.
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
        let Some(last) = rows.last().map(|r| r.id) else {
            return Ok(true);
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
        let body = serde_json::to_vec(&DeliverRequest { items }).map_err(ServiceError::internal)?;
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
            let failures = delivery.failures.saturating_add(1);
            let backoff = 1_i64
                .checked_shl(failures.clamp(1, 16) as u32 - 1)
                .unwrap_or(MAX_BACKOFF_SECS)
                .min(MAX_BACKOFF_SECS);
            self.repos
                .host_repo
                .record_failure(peer, failures, Utc::now() + Duration::seconds(backoff))
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
        let acked = answer
            .acked_through
            .parse::<i64>()
            .unwrap_or(delivery.delivered_through)
            .min(last);
        self.repos.host_repo.record_delivered(peer, acked).await?;
        info!(peer_server_id = %peer, acked_through = acked, refused = answer.refused.len(), "delivered host events");
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::conversation::model::ConversationId;
    use rim_core::server_member::model::ServerMemberRole;

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
}
