//! Metrics recorded by the services through the `metrics` facade. Without an installed recorder
//! every call is a no-op; the server installs the Prometheus exporter when metrics are on.

use metrics::{Unit, counter, describe_counter, describe_gauge, gauge};
use rim_core::relay::model::RelayQueueStats;
use rim_core::relay::repository::RelayRepository;
use rim_core::repository::error::RepoResult;

/// Requests a rate limiter or quota refused, labelled `limiter`.
pub const RATE_LIMIT_REJECTIONS: &str = "rim_rate_limit_rejections_total";
/// Key-only signups refused for a proof-of-work that does not meet the required bits.
pub const POW_REJECTIONS: &str = "rim_pow_rejections_total";
/// Relay queue items, labelled `state` (`queued` or `held`).
pub const RELAY_QUEUE_ITEMS: &str = "rim_relay_queue_items";
/// Relay queue bytes, labelled `state` (`queued` or `held`).
pub const RELAY_QUEUE_BYTES: &str = "rim_relay_queue_bytes";
/// Audit log rows deleted by the retention purge.
pub const AUDIT_ROWS_PURGED: &str = "rim_audit_rows_purged_total";
/// The configured audit retention in days, 0 when rows are kept forever.
pub const AUDIT_RETENTION_DAYS: &str = "rim_audit_retention_days";

/// Register the help text and units of the metrics above with the installed recorder.
pub fn describe() {
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
}

/// Count one request the limiter named `limiter` refused.
pub fn rate_limited(limiter: &'static str) {
    counter!(RATE_LIMIT_REJECTIONS, "limiter" => limiter).increment(1);
}

/// Count one refused proof-of-work.
pub fn pow_rejected() {
    counter!(POW_REJECTIONS).increment(1);
}

/// Count `n` audit log rows the retention purge deleted.
pub fn audit_rows_purged(n: u64) {
    counter!(AUDIT_ROWS_PURGED).increment(n);
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
    use rim_core::relay::repository::MockRelayRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind};

    use super::test_support::{block_on, recorded};
    use super::*;

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
