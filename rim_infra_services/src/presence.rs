use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;

use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_services::events::{EventObserver, Observers, WorkspaceEvent};
use rim_services::presence::{PresenceStatus, UserPresence};

struct PresenceEntry {
    status: PresenceStatus,
    last_heartbeat_at: DateTime<Utc>,
    workspace_ids: Vec<ServerId>,
}

pub struct PresenceManager {
    states: DashMap<UserId, PresenceEntry>,
    observers: Arc<Observers>,
}

/// Minimum interval between accepted heartbeats per user (10 seconds).
const HEARTBEAT_RATE_LIMIT_SECS: i64 = 10;

/// Seconds without heartbeat before marking Online -> Away.
const AWAY_TIMEOUT_SECS: i64 = 90;

/// Seconds without heartbeat before marking -> Offline.
const OFFLINE_TIMEOUT_SECS: i64 = 180;

impl PresenceManager {
    pub fn new(observers: Arc<Observers>) -> Self {
        Self {
            states: DashMap::new(),
            observers,
        }
    }

    /// Process a heartbeat from a client.
    /// Rate-limited: ignores heartbeats arriving < 10s from the previous one.
    /// Only accepts Online/Away — never Offline from clients.
    pub fn heartbeat(&self, user_id: UserId, workspace_id: ServerId, status: PresenceStatus) {
        // CSO: Only accept Online/Away from clients, never Offline.
        if status == PresenceStatus::Offline {
            return;
        }

        let now = Utc::now();

        let status_changed = if let Some(mut entry) = self.states.get_mut(&user_id) {
            // Rate limit: ignore heartbeat if < 10s since last.
            let elapsed = now
                .signed_duration_since(entry.last_heartbeat_at)
                .num_seconds();
            if elapsed < HEARTBEAT_RATE_LIMIT_SECS {
                return;
            }

            let old_status = entry.status;
            entry.last_heartbeat_at = now;
            entry.status = status;

            // Add workspace if not already tracked.
            if !entry.workspace_ids.contains(&workspace_id) {
                entry.workspace_ids.push(workspace_id);
            }

            old_status != status
        } else {
            // New entry — always counts as a change (from implicit Offline).
            self.states.insert(
                user_id,
                PresenceEntry {
                    status,
                    last_heartbeat_at: now,
                    workspace_ids: vec![workspace_id],
                },
            );
            true
        };

        if status_changed {
            self.emit_presence_changed(user_id, status, workspace_id);
        }
    }

    /// Return presence for all users that are part of a given workspace.
    pub fn get_workspace_presence(&self, workspace_id: ServerId) -> Vec<UserPresence> {
        self.states
            .iter()
            .filter(|entry| entry.value().workspace_ids.contains(&workspace_id))
            .map(|entry| UserPresence {
                user_id: *entry.key(),
                status: entry.value().status,
                last_seen_at: entry.value().last_heartbeat_at,
            })
            .collect()
    }

    /// Mark a user as Offline. Called on WS disconnect.
    pub fn mark_offline(&self, user_id: UserId) {
        if let Some(mut entry) = self.states.get_mut(&user_id) {
            if entry.status == PresenceStatus::Offline {
                return;
            }
            entry.status = PresenceStatus::Offline;
            entry.last_heartbeat_at = Utc::now();

            let workspace_ids: Vec<ServerId> = entry.workspace_ids.clone();
            drop(entry);

            for ws_id in workspace_ids {
                self.emit_presence_changed(user_id, PresenceStatus::Offline, ws_id);
            }
        }
    }

    /// Background task: check timeouts and transition states.
    /// 90s without heartbeat: Online -> Away
    /// 180s without heartbeat: -> Offline
    pub fn check_timeouts(&self) {
        let now = Utc::now();

        // Collect users that need state changes to avoid holding DashMap refs while emitting.
        let mut transitions: Vec<(UserId, PresenceStatus, Vec<ServerId>)> = Vec::new();

        for mut entry in self.states.iter_mut() {
            let elapsed = now
                .signed_duration_since(entry.value().last_heartbeat_at)
                .num_seconds();

            let current = entry.value().status;

            if elapsed >= OFFLINE_TIMEOUT_SECS && current != PresenceStatus::Offline {
                entry.status = PresenceStatus::Offline;
                transitions.push((
                    *entry.key(),
                    PresenceStatus::Offline,
                    entry.value().workspace_ids.clone(),
                ));
            } else if elapsed >= AWAY_TIMEOUT_SECS && current == PresenceStatus::Online {
                entry.status = PresenceStatus::Away;
                transitions.push((
                    *entry.key(),
                    PresenceStatus::Away,
                    entry.value().workspace_ids.clone(),
                ));
            }
        }

        for (user_id, new_status, workspace_ids) in transitions {
            for ws_id in workspace_ids {
                self.emit_presence_changed(user_id, new_status, ws_id);
            }
        }

        // Phase 2: Remove entries that have been Offline for > 10 minutes.
        let stale_threshold = now - chrono::Duration::minutes(10);
        let stale_keys: Vec<UserId> = self
            .states
            .iter()
            .filter(|entry| {
                entry.value().status == PresenceStatus::Offline
                    && entry.value().last_heartbeat_at < stale_threshold
            })
            .map(|entry| *entry.key())
            .collect();

        for key in stale_keys {
            self.states.remove(&key);
        }
    }

    fn emit_presence_changed(
        &self,
        user_id: UserId,
        status: PresenceStatus,
        workspace_id: ServerId,
    ) {
        self.observers
            .workspace
            .notify(WorkspaceEvent::PresenceChanged {
                workspace_id,
                user_id,
                status: status.as_str().to_string(),
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_services::events::{NoopBroadcaster, Observers};

    fn make_manager() -> PresenceManager {
        let broadcaster: Arc<dyn rim_services::events::Broadcaster> = Arc::new(NoopBroadcaster);
        let observers = Arc::new(Observers::new(broadcaster));
        PresenceManager::new(observers)
    }

    #[test]
    fn heartbeat_registers_user_online() {
        let mgr = make_manager();
        let uid = UserId::new();
        let ws = ServerId::new();

        mgr.heartbeat(uid, ws, PresenceStatus::Online);

        let presence = mgr.get_workspace_presence(ws);
        assert_eq!(presence.len(), 1);
        assert_eq!(presence[0].user_id, uid);
        assert_eq!(presence[0].status, PresenceStatus::Online);
    }

    #[test]
    fn heartbeat_rejects_offline_from_client() {
        let mgr = make_manager();
        let uid = UserId::new();
        let ws = ServerId::new();

        mgr.heartbeat(uid, ws, PresenceStatus::Offline);

        let presence = mgr.get_workspace_presence(ws);
        assert!(presence.is_empty());
    }

    #[test]
    fn mark_offline_sets_user_offline() {
        let mgr = make_manager();
        let uid = UserId::new();
        let ws = ServerId::new();

        mgr.heartbeat(uid, ws, PresenceStatus::Online);
        mgr.mark_offline(uid);

        let presence = mgr.get_workspace_presence(ws);
        assert_eq!(presence.len(), 1);
        assert_eq!(presence[0].status, PresenceStatus::Offline);
    }

    #[test]
    fn get_workspace_presence_filters_by_workspace() {
        let mgr = make_manager();
        let uid1 = UserId::new();
        let uid2 = UserId::new();
        let ws1 = ServerId::new();
        let ws2 = ServerId::new();

        mgr.heartbeat(uid1, ws1, PresenceStatus::Online);
        mgr.heartbeat(uid2, ws2, PresenceStatus::Online);

        let p1 = mgr.get_workspace_presence(ws1);
        assert_eq!(p1.len(), 1);
        assert_eq!(p1[0].user_id, uid1);

        let p2 = mgr.get_workspace_presence(ws2);
        assert_eq!(p2.len(), 1);
        assert_eq!(p2[0].user_id, uid2);
    }

    #[test]
    fn check_timeouts_transitions_to_away_and_offline() {
        let mgr = make_manager();
        let uid = UserId::new();
        let ws = ServerId::new();

        // Insert with a stale timestamp
        mgr.states.insert(
            uid,
            PresenceEntry {
                status: PresenceStatus::Online,
                last_heartbeat_at: Utc::now() - chrono::Duration::seconds(100),
                workspace_ids: vec![ws],
            },
        );

        mgr.check_timeouts();
        let p = mgr.get_workspace_presence(ws);
        assert_eq!(p[0].status, PresenceStatus::Away);

        // Make it even older for offline
        mgr.states.get_mut(&uid).unwrap().last_heartbeat_at =
            Utc::now() - chrono::Duration::seconds(200);
        mgr.states.get_mut(&uid).unwrap().status = PresenceStatus::Away;

        mgr.check_timeouts();
        let p = mgr.get_workspace_presence(ws);
        assert_eq!(p[0].status, PresenceStatus::Offline);
    }

    #[test]
    fn mark_offline_is_idempotent() {
        let mgr = make_manager();
        let uid = UserId::new();
        let ws = ServerId::new();

        mgr.heartbeat(uid, ws, PresenceStatus::Online);
        mgr.mark_offline(uid);
        mgr.mark_offline(uid); // should not panic or double-emit

        let presence = mgr.get_workspace_presence(ws);
        assert_eq!(presence[0].status, PresenceStatus::Offline);
    }

    #[test]
    fn mark_offline_for_unknown_user_is_noop() {
        let mgr = make_manager();
        mgr.mark_offline(UserId::new()); // should not panic
    }
}
