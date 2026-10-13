//! The in-process registry of connections watching their session, and the recheck that ends
//! the connections of sessions ended where the registry did not hear of it: in another process,
//! by expiry, or by a purge.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use tokio::sync::{oneshot, watch};
use tracing::{error, info};

use rim_core::auth::model::SessionId;
use rim_services::ServiceResult;
use rim_services::auth::service::RequestAuthenticator;
use rim_services::session_revocation::{
    EndedSessions, SessionRevocations, SessionWatch, WatchedSession,
};

/// Most sessions one recheck query asks about.
const RECHECK_BATCH: usize = 500;

/// Connections of this process watching their sessions. A revocation resolves the watch of
/// every connection it names and forgets them.
#[derive(Default)]
pub struct InProcessSessionRevocations {
    watchers: Arc<Mutex<Watchers>>,
}

#[derive(Default)]
struct Watchers {
    next: u64,
    by_id: HashMap<u64, (WatchedSession, oneshot::Sender<()>)>,
}

fn lock(watchers: &Mutex<Watchers>) -> MutexGuard<'_, Watchers> {
    watchers.lock().unwrap_or_else(PoisonError::into_inner)
}

impl SessionRevocations for InProcessSessionRevocations {
    fn end(&self, ended: EndedSessions) {
        let senders: Vec<oneshot::Sender<()>> = {
            let mut watchers = lock(&self.watchers);
            let ids: Vec<u64> = watchers
                .by_id
                .iter()
                .filter(|(_, (session, _))| ended.covers(session))
                .map(|(id, _)| *id)
                .collect();
            ids.iter()
                .filter_map(|id| watchers.by_id.remove(id))
                .map(|(_, sender)| sender)
                .collect()
        };
        if senders.is_empty() {
            return;
        }
        let connections = senders.len();
        for sender in senders {
            let _ = sender.send(());
        }
        info!(connections, "ended the live connections of ended sessions");
    }

    fn watch(&self, session: WatchedSession) -> SessionWatch {
        let (sender, ended) = oneshot::channel();
        let id = {
            let mut watchers = lock(&self.watchers);
            let id = watchers.next;
            watchers.next += 1;
            watchers.by_id.insert(id, (session, sender));
            id
        };
        SessionWatch::new(
            ended,
            Registration {
                watchers: Arc::downgrade(&self.watchers),
                id,
            },
        )
    }

    fn watched(&self) -> Vec<SessionId> {
        let watchers = lock(&self.watchers);
        let ids: HashSet<SessionId> = watchers
            .by_id
            .values()
            .map(|(session, _)| session.session_id)
            .collect();
        ids.into_iter().collect()
    }
}

/// A registry of its own, which no connection watches: the default of a service until the
/// composition root hands it the shared one, and all a command-line process needs.
pub(crate) fn unshared() -> Arc<dyn SessionRevocations> {
    Arc::new(InProcessSessionRevocations::default())
}

/// One watch in the registry, removed when dropped.
struct Registration {
    watchers: Weak<Mutex<Watchers>>,
    id: u64,
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(watchers) = self.watchers.upgrade() {
            lock(&watchers).by_id.remove(&self.id);
        }
    }
}

/// End the connections of every watched session that is no longer live: revoked, expired or
/// gone. Returns how many sessions ended. One query per [`RECHECK_BATCH`] watched sessions.
pub async fn recheck(
    authenticator: &dyn RequestAuthenticator,
    revocations: &dyn SessionRevocations,
) -> ServiceResult<usize> {
    let watched = revocations.watched();
    let mut ended = 0;
    for batch in watched.chunks(RECHECK_BATCH) {
        let live: HashSet<SessionId> = authenticator
            .live_sessions(batch)
            .await?
            .into_iter()
            .collect();
        let gone: Vec<SessionId> = batch
            .iter()
            .filter(|id| !live.contains(id))
            .copied()
            .collect();
        ended += gone.len();
        revocations.revoke(&gone);
    }
    Ok(ended)
}

/// Run [`recheck`] every `interval`, first one interval after the start, until `shutdown`
/// changes. A failed run is logged and leaves the connections open.
pub async fn run_recheck(
    authenticator: Arc<dyn RequestAuthenticator>,
    revocations: Arc<dyn SessionRevocations>,
    interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if let Err(e) = recheck(authenticator.as_ref(), revocations.as_ref()).await {
                    error!(error = %e, "could not recheck the sessions of live connections");
                }
            }
            _ = shutdown.changed() => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use rim_core::key_identity::model::DeviceKeyId;
    use rim_core::user::model::UserId;
    use rim_services::ServiceError;
    use rim_services::auth::service::MockRequestAuthenticator;

    use super::*;

    fn session(user_id: UserId, device_key_id: Option<DeviceKeyId>) -> WatchedSession {
        WatchedSession {
            session_id: SessionId::new(),
            user_id,
            device_key_id,
        }
    }

    /// Two connections of one user, one bound to a device, and one of another user.
    struct Fixture {
        registry: InProcessSessionRevocations,
        user: UserId,
        device: DeviceKeyId,
        password: (WatchedSession, SessionWatch),
        on_device: (WatchedSession, SessionWatch),
        other: (WatchedSession, SessionWatch),
    }

    impl Fixture {
        fn new() -> Self {
            let registry = InProcessSessionRevocations::default();
            let user = UserId::new();
            let device = DeviceKeyId::new();
            let watch = |s: WatchedSession| (s, registry.watch(s));
            let password = watch(session(user, None));
            let on_device = watch(session(user, Some(device)));
            let other = watch(session(UserId::new(), None));
            Self {
                registry,
                user,
                device,
                password,
                on_device,
                other,
            }
        }

        /// Which of password, on-device and other have ended.
        fn ended(&mut self) -> [bool; 3] {
            [
                self.password.1.has_ended(),
                self.on_device.1.has_ended(),
                self.other.1.has_ended(),
            ]
        }
    }

    #[test]
    fn a_revocation_resolves_exactly_the_watches_it_names() {
        let mut f = Fixture::new();
        f.registry.revoke(&[f.password.0.session_id]);
        assert_eq!(f.ended(), [true, false, false]);

        let mut f = Fixture::new();
        f.registry.revoke_device(f.device);
        assert_eq!(f.ended(), [false, true, false]);

        let mut f = Fixture::new();
        f.registry
            .revoke_user_except(f.user, f.password.0.session_id);
        assert_eq!(f.ended(), [false, true, false]);

        let mut f = Fixture::new();
        f.registry.revoke_user(f.user);
        assert_eq!(f.ended(), [true, true, false]);
        assert_eq!(f.registry.watched(), vec![f.other.0.session_id]);
    }

    #[tokio::test]
    async fn a_watch_resolves_as_a_future_once_its_session_is_revoked() {
        let f = Fixture::new();
        let (watched, watch) = f.password;
        let waiter = tokio::spawn(watch);
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        f.registry.revoke(&[watched.session_id]);
        tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("the watch resolves")
            .expect("join");
    }

    #[test]
    fn a_dropped_watch_stops_watching_and_each_session_is_listed_once() {
        let registry = InProcessSessionRevocations::default();
        let shared = session(UserId::new(), None);
        let first = registry.watch(shared);
        let second = registry.watch(shared);
        assert_eq!(registry.watched(), vec![shared.session_id]);

        drop(first);
        assert_eq!(registry.watched(), vec![shared.session_id]);
        drop(second);
        assert!(registry.watched().is_empty());
        // Nothing left to resolve.
        registry.revoke(&[shared.session_id]);
    }

    #[test]
    fn a_watch_outliving_its_registry_stays_pending() {
        let registry = InProcessSessionRevocations::default();
        let mut watch = registry.watch(session(UserId::new(), None));
        drop(registry);
        assert!(!watch.has_ended());
    }

    #[tokio::test]
    async fn the_recheck_ends_the_watched_sessions_that_are_no_longer_live() {
        let mut f = Fixture::new();
        let live = vec![f.on_device.0.session_id, f.other.0.session_id];
        let mut authenticator = MockRequestAuthenticator::new();
        authenticator
            .expect_live_sessions()
            .times(1)
            .withf(|ids| ids.len() == 3)
            .returning(move |_| {
                let live = live.clone();
                Box::pin(async move { Ok(live) })
            });

        let ended = recheck(&authenticator, &f.registry).await.expect("recheck");
        assert_eq!(ended, 1);
        assert_eq!(f.ended(), [true, false, false]);
    }

    #[tokio::test]
    async fn a_failed_recheck_leaves_every_connection_open() {
        let mut f = Fixture::new();
        let mut authenticator = MockRequestAuthenticator::new();
        authenticator
            .expect_live_sessions()
            .returning(|_| Box::pin(async { Err(ServiceError::Unavailable) }));

        assert!(recheck(&authenticator, &f.registry).await.is_err());
        assert_eq!(f.ended(), [false, false, false]);
    }

    #[tokio::test]
    async fn without_watched_sessions_the_recheck_asks_nothing() {
        let registry = InProcessSessionRevocations::default();
        let mut authenticator = MockRequestAuthenticator::new();
        authenticator.expect_live_sessions().never();
        assert_eq!(
            recheck(&authenticator, &registry).await.expect("recheck"),
            0
        );
    }
}
