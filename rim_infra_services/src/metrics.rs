//! Metrics recorded by the services through the `metrics` facade. Without an installed recorder
//! every call is a no-op; the server installs the Prometheus exporter when metrics are on.

use chrono::{Duration, Utc};
use metrics::{Unit, counter, describe_counter, describe_gauge, gauge};
use rim_core::host::model::OutboxStats;
use rim_core::host::repository::HostRepository;
use rim_core::relay::model::RelayQueueStats;
use rim_core::relay::repository::RelayRepository;
use rim_core::repository::error::RepoResult;

/// Newly committed messages, labelled `encryption` (`plaintext` or `encrypted`).
pub const MESSAGES_CREATED: &str = "rim_messages_created_total";
/// Successful local account registrations.
pub const REGISTRATIONS: &str = "rim_registrations_total";

/// Requests a rate limiter or quota refused, labelled `limiter`.
pub const RATE_LIMIT_REJECTIONS: &str = "rim_rate_limit_rejections_total";
/// Key-only signups refused for a proof-of-work that does not meet the required bits.
pub const POW_REJECTIONS: &str = "rim_pow_rejections_total";
/// Relay queue items, labelled `state` (`queued` or `held`).
pub const RELAY_QUEUE_ITEMS: &str = "rim_relay_queue_items";
/// Relay queue bytes, labelled `state` (`queued` or `held`).
pub const RELAY_QUEUE_BYTES: &str = "rim_relay_queue_bytes";
/// Outbox rows no relay has acknowledged yet.
pub const OUTBOX_PENDING_ROWS: &str = "rim_outbox_pending_rows";
/// Bytes of those rows.
pub const OUTBOX_PENDING_BYTES: &str = "rim_outbox_pending_bytes";
/// The most outbox rows one relay has waiting.
pub const OUTBOX_PEER_MAX_PENDING_ROWS: &str = "rim_outbox_peer_max_pending_rows";
/// The most outbox bytes one relay has waiting.
pub const OUTBOX_PEER_MAX_PENDING_BYTES: &str = "rim_outbox_peer_max_pending_bytes";
/// Relays every delivery to has failed for the dead-peer period.
pub const OUTBOX_DEAD_PEERS: &str = "rim_outbox_dead_peers";
/// Host events not queued for a relay, labelled `reason` (`peer_cap` or `dead_peer`).
pub const OUTBOX_NOT_QUEUED: &str = "rim_outbox_not_queued_rows_total";
/// Audit log rows deleted by the retention purge.
pub const AUDIT_ROWS_PURGED: &str = "rim_audit_rows_purged_total";
/// The configured audit retention in days, 0 when rows are kept forever.
pub const AUDIT_RETENTION_DAYS: &str = "rim_audit_retention_days";
/// Stale attachment uploads the daily cleanup marked failed.
pub const ATTACHMENTS_CLEANED: &str = "rim_attachments_cleaned_total";
/// Expired workspace state entries the hourly purge deleted.
pub const WORKSPACE_STATE_PURGED: &str = "rim_workspace_state_purged_total";
/// Expired conversation invitations the hourly purge deleted.
pub const CONVERSATION_INVITES_EXPIRED: &str = "rim_conversation_invites_expired_total";
/// Stale cached remote identities the hourly purge deleted.
pub const REMOTE_IDENTITIES_PURGED: &str = "rim_remote_identities_purged_total";
/// Idle peer pins the hourly purge forgot.
pub const PEER_PINS_FORGOTTEN: &str = "rim_peer_pins_forgotten_total";
/// Revoked and expired sessions the hourly purge deleted.
pub const SESSIONS_PURGED: &str = "rim_sessions_purged_total";

/// Register the help text and units of the metrics above with the installed recorder.
pub fn describe() {
    describe_counter!(
        MESSAGES_CREATED,
        Unit::Count,
        "New messages committed, including replies and forwards, by encryption."
    );
    describe_counter!(
        REGISTRATIONS,
        Unit::Count,
        "Successful local account registrations."
    );
    counter!(REGISTRATIONS).absolute(0);
    for encryption in ["plaintext", "encrypted"] {
        counter!(MESSAGES_CREATED, "encryption" => encryption).absolute(0);
    }
    describe_counter!(
        RATE_LIMIT_REJECTIONS,
        Unit::Count,
        "Requests a rate limiter or quota refused, by limiter."
    );
    describe_counter!(
        POW_REJECTIONS,
        Unit::Count,
        "Key-only signups refused for an insufficient proof-of-work."
    );
    describe_gauge!(
        RELAY_QUEUE_ITEMS,
        Unit::Count,
        "Items in the relay queue, waiting for devices (queued) or behind a DM request (held)."
    );
    describe_gauge!(
        RELAY_QUEUE_BYTES,
        Unit::Bytes,
        "Bytes in the relay queue, waiting for devices (queued) or behind a DM request (held)."
    );
    describe_gauge!(
        OUTBOX_PENDING_ROWS,
        Unit::Count,
        "Outbox rows no relay has acknowledged yet."
    );
    describe_gauge!(
        OUTBOX_PENDING_BYTES,
        Unit::Bytes,
        "Bytes of outbox rows no relay has acknowledged yet."
    );
    describe_gauge!(
        OUTBOX_PEER_MAX_PENDING_ROWS,
        Unit::Count,
        "The most outbox rows one relay has waiting."
    );
    describe_gauge!(
        OUTBOX_PEER_MAX_PENDING_BYTES,
        Unit::Bytes,
        "The most outbox bytes one relay has waiting."
    );
    describe_gauge!(
        OUTBOX_DEAD_PEERS,
        Unit::Count,
        "Relays every delivery to has failed for the dead-peer period."
    );
    describe_counter!(
        OUTBOX_NOT_QUEUED,
        Unit::Count,
        "Host events not queued for a relay, by reason."
    );
    for reason in ["peer_cap", "dead_peer"] {
        counter!(OUTBOX_NOT_QUEUED, "reason" => reason).increment(0);
    }
    describe_counter!(
        AUDIT_ROWS_PURGED,
        Unit::Count,
        "Audit log rows deleted by the retention purge."
    );
    describe_gauge!(
        AUDIT_RETENTION_DAYS,
        Unit::Count,
        "Days audit log rows are kept before the purge; 0 keeps them forever."
    );
    describe_counter!(
        ATTACHMENTS_CLEANED,
        Unit::Count,
        "Stale attachment uploads the daily cleanup marked failed."
    );
    describe_counter!(
        WORKSPACE_STATE_PURGED,
        Unit::Count,
        "Expired workspace state entries the hourly purge deleted."
    );
    describe_counter!(
        CONVERSATION_INVITES_EXPIRED,
        Unit::Count,
        "Expired conversation invitations the hourly purge deleted."
    );
    describe_counter!(
        REMOTE_IDENTITIES_PURGED,
        Unit::Count,
        "Stale cached remote identities the hourly purge deleted."
    );
    describe_counter!(
        PEER_PINS_FORGOTTEN,
        Unit::Count,
        "Idle peer pins the hourly purge forgot."
    );
    describe_counter!(
        SESSIONS_PURGED,
        Unit::Count,
        "Revoked and expired sessions the hourly purge deleted."
    );
}

/// Count one newly committed message.
pub(crate) fn message_created(encrypted: bool) {
    let encryption = if encrypted { "encrypted" } else { "plaintext" };
    counter!(MESSAGES_CREATED, "encryption" => encryption).increment(1);
}

/// Count one successfully committed local account registration.
pub(crate) fn registered() {
    counter!(REGISTRATIONS).increment(1);
}

/// Count one request the limiter named `limiter` refused.
pub fn rate_limited(limiter: &'static str) {
    counter!(RATE_LIMIT_REJECTIONS, "limiter" => limiter).increment(1);
}

/// Count one refused proof-of-work.
pub fn pow_rejected() {
    counter!(POW_REJECTIONS).increment(1);
}

/// Count one host event not queued for a relay, for `reason` (`peer_cap` or `dead_peer`).
pub(crate) fn outbox_not_queued(reason: &'static str) {
    counter!(OUTBOX_NOT_QUEUED, "reason" => reason).increment(1);
}

/// Count `n` audit log rows the retention purge deleted.
pub fn audit_rows_purged(n: u64) {
    counter!(AUDIT_ROWS_PURGED).increment(n);
}

/// Count `n` stale attachment uploads the cleanup marked failed.
pub fn attachments_cleaned(n: u64) {
    counter!(ATTACHMENTS_CLEANED).increment(n);
}

/// Count `n` expired workspace state entries the purge deleted.
pub fn workspace_state_purged(n: u64) {
    counter!(WORKSPACE_STATE_PURGED).increment(n);
}

/// Count `n` expired conversation invitations the purge deleted.
pub fn conversation_invites_expired(n: u64) {
    counter!(CONVERSATION_INVITES_EXPIRED).increment(n);
}

/// Count `n` stale cached remote identities the purge deleted.
pub fn remote_identities_purged(n: u64) {
    counter!(REMOTE_IDENTITIES_PURGED).increment(n);
}

/// Count `n` idle peer pins the purge forgot.
pub fn peer_pins_forgotten(n: u64) {
    counter!(PEER_PINS_FORGOTTEN).increment(n);
}

/// Count `n` revoked or expired sessions the purge deleted.
pub fn sessions_purged(n: u64) {
    counter!(SESSIONS_PURGED).increment(n);
}

/// Set the audit retention gauge; `None` (keep forever) is recorded as 0.
pub fn set_audit_retention_days(days: Option<u32>) {
    gauge!(AUDIT_RETENTION_DAYS).set(f64::from(days.unwrap_or(0)));
}

/// Set the relay queue gauges from `stats`.
pub fn record_relay_queue(stats: RelayQueueStats) {
    let as_gauge = |n: i64| n.max(0) as f64;
    gauge!(RELAY_QUEUE_ITEMS, "state" => "queued").set(as_gauge(stats.queued_items));
    gauge!(RELAY_QUEUE_BYTES, "state" => "queued").set(as_gauge(stats.queued_bytes));
    gauge!(RELAY_QUEUE_ITEMS, "state" => "held").set(as_gauge(stats.held_items));
    gauge!(RELAY_QUEUE_BYTES, "state" => "held").set(as_gauge(stats.held_bytes));
}

/// Read the relay queue totals and set the gauges; on error the gauges keep their last value.
pub async fn sample_relay_queue(repo: &dyn RelayRepository) -> RepoResult<()> {
    record_relay_queue(repo.queue_stats().await?);
    Ok(())
}

/// Set the outbox gauges from `stats`.
pub fn record_outbox(stats: OutboxStats) {
    let as_gauge = |n: i64| n.max(0) as f64;
    gauge!(OUTBOX_PENDING_ROWS).set(as_gauge(stats.rows));
    gauge!(OUTBOX_PENDING_BYTES).set(as_gauge(stats.bytes));
    gauge!(OUTBOX_PEER_MAX_PENDING_ROWS).set(as_gauge(stats.peer_max_rows));
    gauge!(OUTBOX_PEER_MAX_PENDING_BYTES).set(as_gauge(stats.peer_max_bytes));
    gauge!(OUTBOX_DEAD_PEERS).set(as_gauge(stats.dead_peers));
}

/// Read the outbox totals, counting relays dead after `dead_after` of failures, and set the
/// gauges; on error the gauges keep their last value.
pub async fn sample_outbox(repo: &dyn HostRepository, dead_after: Duration) -> RepoResult<()> {
    record_outbox(repo.outbox_stats(Utc::now() - dead_after).await?);
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    use metrics::Key;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use metrics_util::{CompositeKey, MetricKind};

    /// Run `f` with a recorder of its own on this thread and return what it recorded. Async
    /// code runs inside `f` on a current-thread runtime, so it records into the same recorder.
    pub(crate) fn recorded<T>(f: impl FnOnce() -> T) -> (T, Recorded) {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let out = metrics::with_local_recorder(&recorder, f);
        let values = snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .map(|(key, _, _, value)| (key, value))
            .collect();
        (out, Recorded(values))
    }

    /// Block on `fut` on a fresh current-thread runtime.
    pub(crate) fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(fut)
    }

    /// The metrics a closure recorded, read once from the recorder.
    pub(crate) struct Recorded(Vec<(CompositeKey, DebugValue)>);

    impl Recorded {
        pub(crate) fn snapshot(snapshotter: &metrics_util::debugging::Snapshotter) -> Self {
            Self(
                snapshotter
                    .snapshot()
                    .into_vec()
                    .into_iter()
                    .map(|(key, _, _, value)| (key, value))
                    .collect(),
            )
        }

        fn value(
            &self,
            kind: MetricKind,
            name: &str,
            labels: &[(&str, &str)],
        ) -> Option<&DebugValue> {
            let labels: Vec<metrics::Label> = labels
                .iter()
                .map(|(k, v)| metrics::Label::new(k.to_string(), v.to_string()))
                .collect();
            let wanted = CompositeKey::new(kind, Key::from_parts(name.to_string(), labels));
            self.0
                .iter()
                .find(|(key, _)| *key == wanted)
                .map(|(_, value)| value)
        }

        /// The value of a counter, 0 when it was never touched.
        pub(crate) fn counter(&self, name: &str, labels: &[(&str, &str)]) -> u64 {
            match self.value(MetricKind::Counter, name, labels) {
                Some(DebugValue::Counter(n)) => *n,
                _ => 0,
            }
        }

        /// The value of a gauge, if it was set.
        pub(crate) fn gauge(&self, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
            match self.value(MetricKind::Gauge, name, labels) {
                Some(DebugValue::Gauge(v)) => Some(v.into_inner()),
                _ => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rim_core::host::repository::MockHostRepository;
    use rim_core::relay::repository::MockRelayRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind};

    use super::test_support::{block_on, recorded};
    use super::*;

    #[test]
    fn application_counters_start_at_zero_and_bootstrap_preserves_existing_events() {
        let ((), got) = recorded(|| {
            describe();
            registered();
            message_created(false);
            describe();
        });
        assert_eq!(got.counter(REGISTRATIONS, &[]), 1);
        assert_eq!(
            got.counter(MESSAGES_CREATED, &[("encryption", "plaintext")]),
            1
        );
        assert_eq!(
            got.counter(MESSAGES_CREATED, &[("encryption", "encrypted")]),
            0
        );
    }

    #[test]
    fn rejections_are_counted_by_limiter() {
        let ((), got) = recorded(|| {
            rate_limited("login");
            rate_limited("login");
            rate_limited("signup");
            pow_rejected();
        });
        assert_eq!(
            got.counter(RATE_LIMIT_REJECTIONS, &[("limiter", "login")]),
            2
        );
        assert_eq!(
            got.counter(RATE_LIMIT_REJECTIONS, &[("limiter", "signup")]),
            1
        );
        assert_eq!(got.counter(POW_REJECTIONS, &[]), 1);
    }

    #[test]
    fn the_audit_retention_metrics_count_rows_and_record_the_setting() {
        let ((), got) = recorded(|| {
            audit_rows_purged(5000);
            audit_rows_purged(12);
            set_audit_retention_days(Some(90));
        });
        assert_eq!(got.counter(AUDIT_ROWS_PURGED, &[]), 5012);
        assert_eq!(got.gauge(AUDIT_RETENTION_DAYS, &[]), Some(90.0));

        let ((), got) = recorded(|| set_audit_retention_days(None));
        assert_eq!(got.gauge(AUDIT_RETENTION_DAYS, &[]), Some(0.0));
    }

    #[test]
    fn the_relay_queue_gauges_come_from_the_repository() {
        let mut repo = MockRelayRepository::new();
        repo.expect_queue_stats().times(1).returning(|| {
            Box::pin(async {
                Ok(RelayQueueStats {
                    queued_items: 3,
                    queued_bytes: 300,
                    held_items: 1,
                    held_bytes: 120,
                })
            })
        });
        let (result, got) = recorded(|| block_on(sample_relay_queue(&repo)));
        assert!(result.is_ok());
        assert_eq!(
            got.gauge(RELAY_QUEUE_ITEMS, &[("state", "queued")]),
            Some(3.0)
        );
        assert_eq!(
            got.gauge(RELAY_QUEUE_BYTES, &[("state", "queued")]),
            Some(300.0)
        );
        assert_eq!(
            got.gauge(RELAY_QUEUE_ITEMS, &[("state", "held")]),
            Some(1.0)
        );
        assert_eq!(
            got.gauge(RELAY_QUEUE_BYTES, &[("state", "held")]),
            Some(120.0)
        );
    }

    #[test]
    fn the_outbox_gauges_come_from_the_repository_and_name_no_relay() {
        let mut repo = MockHostRepository::new();
        repo.expect_outbox_stats()
            .times(1)
            .returning(|dead_before| {
                assert!(dead_before < Utc::now() - Duration::hours(23));
                Box::pin(async {
                    Ok(OutboxStats {
                        rows: 12,
                        bytes: 1200,
                        peer_max_rows: 10,
                        peer_max_bytes: 1000,
                        dead_peers: 1,
                    })
                })
            });
        let (result, got) = recorded(|| block_on(sample_outbox(&repo, Duration::hours(24))));
        assert!(result.is_ok());
        assert_eq!(got.gauge(OUTBOX_PENDING_ROWS, &[]), Some(12.0));
        assert_eq!(got.gauge(OUTBOX_PENDING_BYTES, &[]), Some(1200.0));
        assert_eq!(got.gauge(OUTBOX_PEER_MAX_PENDING_ROWS, &[]), Some(10.0));
        assert_eq!(got.gauge(OUTBOX_PEER_MAX_PENDING_BYTES, &[]), Some(1000.0));
        assert_eq!(got.gauge(OUTBOX_DEAD_PEERS, &[]), Some(1.0));
    }

    #[test]
    fn a_failed_read_leaves_the_relay_queue_gauges_unset() {
        let mut repo = MockRelayRepository::new();
        repo.expect_queue_stats().returning(|| {
            Box::pin(async { Err(RepoError::with_message(RepoErrorKind::Internal, "down")) })
        });
        let (result, got) = recorded(|| block_on(sample_relay_queue(&repo)));
        assert!(result.is_err());
        assert_eq!(got.gauge(RELAY_QUEUE_ITEMS, &[("state", "queued")]), None);
    }
}
