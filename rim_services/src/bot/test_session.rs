//! Webhook test sessions: what reaches a test URL, or a webhook someone is listening on, held in
//! memory for a short time and read only by the person who started the session.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use rim_core::bot::model::{IncomingWebhookId, WebhookTestSessionId};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

/// The most deliveries a session holds; past it the oldest is dropped.
pub const MAX_TEST_DELIVERIES: usize = 20;
/// The most bytes of deliveries a session holds; past it the oldest are dropped.
pub const MAX_TEST_SESSION_BYTES: usize = 1024 * 1024;
/// The most open sessions in one workspace.
pub const MAX_TEST_SESSIONS_PER_WORKSPACE: usize = 3;
/// The most open sessions on the server.
pub const MAX_TEST_SESSIONS: usize = 20;
/// The largest body a session holds the bytes of; a larger one is held as too large.
pub const MAX_TEST_BODY_BYTES: usize = 256 * 1024;
/// How long a session lasts after its start or its last read.
pub const TEST_SESSION_IDLE: Duration = Duration::minutes(15);
/// How long a session lasts at most, from its start.
pub const TEST_SESSION_LIFETIME: Duration = Duration::hours(1);

/// A session as its owner and the server know it. Never its token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSessionInfo {
    pub id: WebhookTestSessionId,
    pub server_id: ServerId,
    /// The person who started it, the only one who reads it.
    pub owner: UserId,
    /// The webhook it copies; `None` for an unbound session with a URL of its own.
    pub webhook_id: Option<IncomingWebhookId>,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// A session to open. An unbound one carries the SHA-256 of its token, never the token. `Debug`
/// leaves the hash out.
#[derive(Clone, PartialEq, Eq)]
pub struct NewTestSession {
    pub server_id: ServerId,
    pub owner: UserId,
    pub webhook_id: Option<IncomingWebhookId>,
    pub token_hash: Option<String>,
}

impl fmt::Debug for NewTestSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewTestSession")
            .field("server_id", &self.server_id)
            .field("owner", &self.owner)
            .field("webhook_id", &self.webhook_id)
            .field("unbound", &self.token_hash.is_some())
            .finish()
    }
}

/// A delivery's body as a session holds it. `Debug` names the variant and the size alone.
#[derive(Clone, PartialEq, Eq)]
pub enum TestBody {
    /// The body exactly as it came.
    Text(String),
    /// Over [`MAX_TEST_BODY_BYTES`]; its bytes are not held.
    TooLarge,
    /// Not UTF-8; its bytes are not held.
    NotText,
}

impl fmt::Debug for TestBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => write!(f, "Text({} bytes)", text.len()),
            Self::TooLarge => f.write_str("TooLarge"),
            Self::NotText => f.write_str("NotText"),
        }
    }
}

/// The names of `headers` and the bytes of their values, for a `Debug` that shows no value.
fn header_sizes(headers: &[(String, String)]) -> Vec<(&str, usize)> {
    headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.len()))
        .collect()
}

/// One delivery a session holds. `seq` counts from 1 within its session. `Debug` shows sizes and
/// header names, never a value or the body.
#[derive(Clone, PartialEq, Eq)]
pub struct TestDelivery {
    pub seq: u64,
    pub received_at: DateTime<Utc>,
    pub content_type: Option<String>,
    /// The `x-` headers a format may use.
    pub headers: Vec<(String, String)>,
    pub body: TestBody,
}

impl fmt::Debug for TestDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestDelivery")
            .field("seq", &self.seq)
            .field("received_at", &self.received_at)
            .field(
                "content_type_bytes",
                &self.content_type.as_ref().map(String::len),
            )
            .field("headers", &header_sizes(&self.headers))
            .field("body", &self.body)
            .finish()
    }
}

impl TestDelivery {
    /// The bytes it takes against [`MAX_TEST_SESSION_BYTES`].
    fn bytes(&self) -> usize {
        let body = match &self.body {
            TestBody::Text(text) => text.len(),
            TestBody::TooLarge | TestBody::NotText => 0,
        };
        body + self.content_type.as_ref().map_or(0, String::len)
            + self
                .headers
                .iter()
                .map(|(name, value)| name.len() + value.len())
                .sum::<usize>()
    }
}

/// What reaches a session, before it is numbered and stamped. `Debug` shows sizes and header
/// names, never a value or the body.
#[derive(Clone, PartialEq, Eq)]
pub struct ArrivedDelivery {
    pub content_type: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: TestBody,
}

impl fmt::Debug for ArrivedDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArrivedDelivery")
            .field(
                "content_type_bytes",
                &self.content_type.as_ref().map(String::len),
            )
            .field("headers", &header_sizes(&self.headers))
            .field("body", &self.body)
            .finish()
    }
}

/// Why a session could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestSessionsFull {
    /// The workspace has [`MAX_TEST_SESSIONS_PER_WORKSPACE`] open.
    Workspace,
    /// The server has [`MAX_TEST_SESSIONS`] open.
    Server,
}

/// Where webhook test sessions are held. [`InMemoryTestSessionStore`] is the only implementation;
/// a store shared by several nodes would implement this trait. Every method takes the time it
/// runs at, and treats a session past its expiry as gone.
pub trait WebhookTestSessionStore: Send + Sync + std::fmt::Debug {
    /// Open a session, unless the workspace or the server is full.
    ///
    /// # Errors
    /// [`TestSessionsFull`] naming which limit was reached.
    fn open(
        &self,
        session: NewTestSession,
        now: DateTime<Utc>,
    ) -> Result<TestSessionInfo, TestSessionsFull>;

    fn get(&self, id: WebhookTestSessionId, now: DateTime<Utc>) -> Option<TestSessionInfo>;

    /// The unbound session whose token has the SHA-256 `token_hash`.
    fn unbound_by_hash(&self, token_hash: &str, now: DateTime<Utc>) -> Option<TestSessionInfo>;

    /// The sessions bound to `webhook_id`.
    fn bound_to(
        &self,
        webhook_id: IncomingWebhookId,
        now: DateTime<Utc>,
    ) -> Vec<WebhookTestSessionId>;

    /// Hold `delivery` in session `id`, dropping the oldest past the limits. Nothing happens for a
    /// session that is gone.
    fn record(&self, id: WebhookTestSessionId, delivery: ArrivedDelivery, now: DateTime<Utc>);

    /// The deliveries after `after`, oldest first, and the expiry, which the read extends.
    fn read(
        &self,
        id: WebhookTestSessionId,
        after: u64,
        now: DateTime<Utc>,
    ) -> Option<(Vec<TestDelivery>, DateTime<Utc>)>;

    /// End an unbound session and answer it with its token's hash, for a webhook to keep. `None`
    /// when it is gone or bound, so it can be taken once.
    fn take_unbound(
        &self,
        id: WebhookTestSessionId,
        now: DateTime<Utc>,
    ) -> Option<(TestSessionInfo, String)>;

    /// End a session at once. Answers it when it was open.
    fn end(&self, id: WebhookTestSessionId) -> Option<TestSessionInfo>;

    /// End every session bound to `webhook_id`, and answer them.
    fn end_bound_to(&self, webhook_id: IncomingWebhookId) -> Vec<TestSessionInfo>;

    /// Drop every session past its expiry. Answers how many.
    fn sweep(&self, now: DateTime<Utc>) -> usize;
}

struct Held {
    info: TestSessionInfo,
    token_hash: Option<String>,
    deliveries: VecDeque<TestDelivery>,
    bytes: usize,
    next_seq: u64,
}

impl fmt::Debug for Held {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Held")
            .field("info", &self.info)
            .field("unbound", &self.token_hash.is_some())
            .field("deliveries", &self.deliveries.len())
            .field("bytes", &self.bytes)
            .finish()
    }
}

/// Sessions in the memory of this process. A restart drops them. `Debug` shows how many there are
/// and nothing they hold.
#[derive(Default)]
pub struct InMemoryTestSessionStore {
    inner: Mutex<HashMap<WebhookTestSessionId, Held>>,
}

impl fmt::Debug for InMemoryTestSessionStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        f.debug_struct("InMemoryTestSessionStore")
            .field("sessions", &guard.values().collect::<Vec<_>>())
            .finish()
    }
}

impl InMemoryTestSessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The sessions, with expired ones dropped first.
    fn sessions(
        &self,
        now: DateTime<Utc>,
    ) -> std::sync::MutexGuard<'_, HashMap<WebhookTestSessionId, Held>> {
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.retain(|_, held| held.info.expires_at > now);
        guard
    }
}

impl WebhookTestSessionStore for InMemoryTestSessionStore {
    fn open(
        &self,
        session: NewTestSession,
        now: DateTime<Utc>,
    ) -> Result<TestSessionInfo, TestSessionsFull> {
        let mut sessions = self.sessions(now);
        if sessions.len() >= MAX_TEST_SESSIONS {
            return Err(TestSessionsFull::Server);
        }
        let in_workspace = sessions
            .values()
            .filter(|held| held.info.server_id == session.server_id)
            .count();
        if in_workspace >= MAX_TEST_SESSIONS_PER_WORKSPACE {
            return Err(TestSessionsFull::Workspace);
        }
        let info = TestSessionInfo {
            id: WebhookTestSessionId::new(),
            server_id: session.server_id,
            owner: session.owner,
            webhook_id: session.webhook_id,
            started_at: now,
            expires_at: now + TEST_SESSION_IDLE,
        };
        sessions.insert(
            info.id,
            Held {
                info: info.clone(),
                token_hash: session.token_hash,
                deliveries: VecDeque::new(),
                bytes: 0,
                next_seq: 1,
            },
        );
        Ok(info)
    }

    fn get(&self, id: WebhookTestSessionId, now: DateTime<Utc>) -> Option<TestSessionInfo> {
        self.sessions(now).get(&id).map(|held| held.info.clone())
    }

    fn unbound_by_hash(&self, token_hash: &str, now: DateTime<Utc>) -> Option<TestSessionInfo> {
        self.sessions(now)
            .values()
            .find(|held| held.token_hash.as_deref() == Some(token_hash))
            .map(|held| held.info.clone())
    }

    fn bound_to(
        &self,
        webhook_id: IncomingWebhookId,
        now: DateTime<Utc>,
    ) -> Vec<WebhookTestSessionId> {
        self.sessions(now)
            .values()
            .filter(|held| held.info.webhook_id == Some(webhook_id))
            .map(|held| held.info.id)
            .collect()
    }

    fn record(&self, id: WebhookTestSessionId, delivery: ArrivedDelivery, now: DateTime<Utc>) {
        let mut sessions = self.sessions(now);
        let Some(held) = sessions.get_mut(&id) else {
            return;
        };
        let delivery = TestDelivery {
            seq: held.next_seq,
            received_at: now,
            content_type: delivery.content_type,
            headers: delivery.headers,
            body: delivery.body,
        };
        held.next_seq += 1;
        held.bytes += delivery.bytes();
        held.deliveries.push_back(delivery);
        while held.deliveries.len() > MAX_TEST_DELIVERIES || held.bytes > MAX_TEST_SESSION_BYTES {
            let Some(oldest) = held.deliveries.pop_front() else {
                break;
            };
            held.bytes -= oldest.bytes();
        }
    }

    fn read(
        &self,
        id: WebhookTestSessionId,
        after: u64,
        now: DateTime<Utc>,
    ) -> Option<(Vec<TestDelivery>, DateTime<Utc>)> {
        let mut sessions = self.sessions(now);
        let held = sessions.get_mut(&id)?;
        held.info.expires_at =
            (now + TEST_SESSION_IDLE).min(held.info.started_at + TEST_SESSION_LIFETIME);
        let deliveries = held
            .deliveries
            .iter()
            .filter(|delivery| delivery.seq > after)
            .cloned()
            .collect();
        Some((deliveries, held.info.expires_at))
    }

    fn take_unbound(
        &self,
        id: WebhookTestSessionId,
        now: DateTime<Utc>,
    ) -> Option<(TestSessionInfo, String)> {
        let mut sessions = self.sessions(now);
        sessions.get(&id)?.token_hash.as_ref()?;
        let held = sessions.remove(&id)?;
        Some((held.info, held.token_hash?))
    }

    fn end(&self, id: WebhookTestSessionId) -> Option<TestSessionInfo> {
        let mut sessions = self.sessions(Utc::now());
        sessions.remove(&id).map(|held| held.info)
    }

    fn end_bound_to(&self, webhook_id: IncomingWebhookId) -> Vec<TestSessionInfo> {
        let mut sessions = self.sessions(Utc::now());
        let ids: Vec<WebhookTestSessionId> = sessions
            .values()
            .filter(|held| held.info.webhook_id == Some(webhook_id))
            .map(|held| held.info.id)
            .collect();
        ids.iter()
            .filter_map(|id| sessions.remove(id).map(|held| held.info))
            .collect()
    }

    fn sweep(&self, now: DateTime<Utc>) -> usize {
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let before = guard.len();
        guard.retain(|_, held| held.info.expires_at > now);
        before - guard.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unbound(server_id: ServerId, hash: &str) -> NewTestSession {
        NewTestSession {
            server_id,
            owner: UserId::new(),
            webhook_id: None,
            token_hash: Some(hash.to_string()),
        }
    }

    fn text(body: &str) -> ArrivedDelivery {
        ArrivedDelivery {
            content_type: None,
            headers: vec![],
            body: TestBody::Text(body.to_string()),
        }
    }

    #[test]
    fn a_workspace_opens_three_and_the_server_twenty() {
        let store = InMemoryTestSessionStore::new();
        let now = Utc::now();
        let busy = ServerId::new();
        for n in 0..MAX_TEST_SESSIONS_PER_WORKSPACE {
            store
                .open(unbound(busy, &format!("b{n}")), now)
                .expect("room");
        }
        assert_eq!(
            store.open(unbound(busy, "one more"), now),
            Err(TestSessionsFull::Workspace)
        );
        while store.inner.lock().expect("lock").len() < MAX_TEST_SESSIONS {
            store
                .open(unbound(ServerId::new(), "x"), now)
                .expect("room");
        }
        assert_eq!(
            store.open(unbound(ServerId::new(), "past"), now),
            Err(TestSessionsFull::Server)
        );
        // An expired one makes room.
        let later = now + TEST_SESSION_IDLE + Duration::seconds(1);
        assert!(store.open(unbound(busy, "again"), later).is_ok());
    }

    #[test]
    fn a_session_holds_twenty_deliveries_dropping_the_oldest() {
        let store = InMemoryTestSessionStore::new();
        let now = Utc::now();
        let id = store
            .open(unbound(ServerId::new(), "h"), now)
            .expect("open")
            .id;
        for n in 0..MAX_TEST_DELIVERIES {
            store.record(id, text(&n.to_string()), now);
        }
        let (held, _) = store.read(id, 0, now).expect("read");
        assert_eq!(held.len(), MAX_TEST_DELIVERIES);
        assert_eq!(held[0].seq, 1);
        store.record(id, text("last"), now);
        let (held, _) = store.read(id, 0, now).expect("read");
        assert_eq!(held.len(), MAX_TEST_DELIVERIES);
        assert_eq!(held[0].seq, 2);
        assert_eq!(
            held.last().expect("last").body,
            TestBody::Text("last".to_string())
        );
        let (after, _) = store.read(id, 20, now).expect("read");
        assert_eq!(after.iter().map(|d| d.seq).collect::<Vec<_>>(), [21]);
    }

    #[test]
    fn a_session_holds_one_mib_dropping_the_oldest() {
        let store = InMemoryTestSessionStore::new();
        let now = Utc::now();
        let id = store
            .open(unbound(ServerId::new(), "h"), now)
            .expect("open")
            .id;
        let quarter = "a".repeat(MAX_TEST_SESSION_BYTES / 4);
        for _ in 0..4 {
            store.record(id, text(&quarter), now);
        }
        assert_eq!(
            store.read(id, 0, now).expect("read").0.len(),
            4,
            "1 MiB exactly is held"
        );
        store.record(id, text("b"), now);
        let (held, _) = store.read(id, 0, now).expect("read");
        assert_eq!(held.iter().map(|d| d.seq).collect::<Vec<_>>(), [2, 3, 4, 5]);
        let total: usize = held.iter().map(TestDelivery::bytes).sum();
        assert!(total <= MAX_TEST_SESSION_BYTES);
    }

    #[test]
    fn a_session_lasts_fifteen_minutes_from_its_last_read_and_an_hour_at_most() {
        let store = InMemoryTestSessionStore::new();
        let start = Utc::now();
        let info = store
            .open(unbound(ServerId::new(), "h"), start)
            .expect("open");
        assert_eq!(info.expires_at, start + TEST_SESSION_IDLE);
        let mut at = start;
        for _ in 0..3 {
            at += Duration::minutes(14);
            let (_, expires) = store.read(info.id, 0, at).expect("still open");
            assert_eq!(expires, at + TEST_SESSION_IDLE);
        }
        at += Duration::minutes(14);
        let (_, expires) = store.read(info.id, 0, at).expect("open");
        assert_eq!(expires, start + TEST_SESSION_LIFETIME, "never past an hour");
        assert!(store.get(info.id, start + Duration::minutes(59)).is_some());
        assert!(store.get(info.id, start + TEST_SESSION_LIFETIME).is_none());

        let idle = store
            .open(unbound(ServerId::new(), "i"), start)
            .expect("open");
        assert!(store.get(idle.id, start + Duration::minutes(14)).is_some());
        assert!(store.get(idle.id, start + Duration::minutes(15)).is_none());
    }

    #[test]
    fn the_sweep_drops_expired_sessions_and_keeps_open_ones() {
        let store = InMemoryTestSessionStore::new();
        let start = Utc::now();
        store
            .open(unbound(ServerId::new(), "old"), start)
            .expect("open");
        let fresh = store
            .open(
                unbound(ServerId::new(), "new"),
                start + Duration::minutes(10),
            )
            .expect("open");
        assert_eq!(store.sweep(start + Duration::minutes(16)), 1);
        assert_eq!(store.inner.lock().expect("lock").len(), 1);
        assert!(store.inner.lock().expect("lock").contains_key(&fresh.id));
        assert_eq!(store.sweep(start + Duration::minutes(16)), 0);
    }

    #[test]
    fn an_unbound_session_is_found_by_its_hash_and_taken_once() {
        let store = InMemoryTestSessionStore::new();
        let now = Utc::now();
        let info = store
            .open(unbound(ServerId::new(), "hash"), now)
            .expect("open");
        assert_eq!(
            store.unbound_by_hash("hash", now).map(|s| s.id),
            Some(info.id)
        );
        assert!(store.unbound_by_hash("other", now).is_none());
        let (taken, hash) = store.take_unbound(info.id, now).expect("taken");
        assert_eq!((taken.id, hash.as_str()), (info.id, "hash"));
        assert!(store.take_unbound(info.id, now).is_none());
        assert!(store.unbound_by_hash("hash", now).is_none());
    }

    #[test]
    fn a_bound_session_is_found_by_its_webhook_and_ended_with_it() {
        let store = InMemoryTestSessionStore::new();
        let now = Utc::now();
        let webhook = IncomingWebhookId::new();
        let bound = store
            .open(
                NewTestSession {
                    server_id: ServerId::new(),
                    owner: UserId::new(),
                    webhook_id: Some(webhook),
                    token_hash: None,
                },
                now,
            )
            .expect("open");
        assert_eq!(store.bound_to(webhook, now), [bound.id]);
        assert!(
            store.take_unbound(bound.id, now).is_none(),
            "a bound one has no URL"
        );
        assert_eq!(store.end_bound_to(webhook), std::slice::from_ref(&bound));
        assert!(store.bound_to(webhook, now).is_empty());
        assert!(store.end(bound.id).is_none());
    }

    #[test]
    fn nothing_a_session_holds_is_formatted() {
        let store = InMemoryTestSessionStore::new();
        let now = Utc::now();
        let new = unbound(ServerId::new(), "marker-token-hash");
        assert!(!format!("{new:?}").contains("marker-"));
        let id = store.open(new, now).expect("open").id;
        let arrived = ArrivedDelivery {
            content_type: Some("marker-content-type".to_string()),
            headers: vec![("x-event".to_string(), "marker-header-value".to_string())],
            body: TestBody::Text("marker-body".to_string()),
        };
        assert!(!format!("{arrived:?}").contains("marker-"));
        store.record(id, arrived, now);
        let (held, _) = store.read(id, 0, now).expect("read");
        for shown in [
            format!("{store:?}"),
            format!("{store:#?}"),
            format!("{held:?}"),
            format!("{:?}", held[0].body),
        ] {
            assert!(!shown.contains("marker-"), "{shown}");
        }
        let shown = format!("{held:?}");
        assert!(shown.contains("x-event"), "header names show: {shown}");
        assert!(shown.contains("Text(11 bytes)"), "{shown}");
    }

    #[test]
    fn a_delivery_to_a_session_that_is_gone_is_dropped() {
        let store = InMemoryTestSessionStore::new();
        let now = Utc::now();
        let id = store
            .open(unbound(ServerId::new(), "h"), now)
            .expect("open")
            .id;
        store.end(id).expect("ended");
        store.record(id, text("late"), now);
        assert!(store.read(id, 0, now).is_none());
    }
}
