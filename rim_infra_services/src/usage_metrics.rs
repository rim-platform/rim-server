use metrics::{Unit, counter, describe_counter, describe_gauge, gauge};

use rim_core::monitoring::MonitoringRepository;
use rim_core::repository::error::RepoResult;

/// Register aggregate usage, pending delivery, and sampling health metrics.
pub fn describe() {
    for (name, text) in [
        (
            "rim_registered_accounts",
            "Current local accounts, excluding remote identities and deleted tombstones.",
        ),
        (
            "rim_active_users",
            "Distinct local accounts with authenticated API activity, by rolling window.",
        ),
        (
            "rim_relay_pending_items",
            "Unexpired unheld recipient items awaiting at least one eligible device acknowledgement.",
        ),
        (
            "rim_relay_pending_device_items",
            "Outstanding item and non-revoked eligible device pairs.",
        ),
        (
            "rim_relay_pending_users",
            "Local accounts with pending device deliveries.",
        ),
        (
            "rim_relay_pending_devices",
            "Non-revoked devices with pending deliveries.",
        ),
        (
            "rim_database_sample_success",
            "Whether the most recent database sample succeeded, by sample.",
        ),
    ] {
        describe_gauge!(name, Unit::Count, text);
    }
    describe_gauge!(
        "rim_relay_pending_bytes",
        Unit::Bytes,
        "Bytes of pending recipient items, counted once per recipient row."
    );
    describe_gauge!(
        "rim_relay_oldest_pending_age_seconds",
        Unit::Seconds,
        "Age of the oldest pending recipient item; zero when empty."
    );
    describe_gauge!(
        "rim_database_sample_last_success_timestamp_seconds",
        Unit::Seconds,
        "Unix timestamp of the most recent successful database sample, by sample."
    );
    describe_counter!(
        "rim_database_sample_failures_total",
        Unit::Count,
        "Database samples that failed or timed out, by sample."
    );
    describe_counter!(
        "rim_relay_enqueued_items_total",
        Unit::Count,
        "Recipient queue rows inserted by committed deliveries, including held items."
    );
    describe_counter!(
        "rim_relay_ack_requests_total",
        Unit::Count,
        "Successful device acknowledgement requests, including repeated cursors."
    );
    describe_counter!(
        "rim_relay_removed_items_total",
        Unit::Count,
        "Queue rows removed by cleanup, by reason; acknowledged cleanup rows are not per-device acknowledgements."
    );
    counter!("rim_relay_enqueued_items_total").increment(0);
    counter!("rim_relay_ack_requests_total").increment(0);
    for reason in ["acknowledged", "expired", "overflow"] {
        counter!("rim_relay_removed_items_total", "reason" => reason).increment(0);
    }
    for sample in ["relay", "outbox", "migrations", "usage", "delivery"] {
        gauge!("rim_database_sample_success", "sample" => sample).set(0.0);
        gauge!("rim_database_sample_last_success_timestamp_seconds", "sample" => sample).set(0.0);
        counter!("rim_database_sample_failures_total", "sample" => sample).increment(0);
    }
}

/// Record a sampler's outcome while preserving previous aggregate values on failure.
pub fn sample_outcome(sample: &'static str, success: bool) {
    gauge!("rim_database_sample_success", "sample" => sample).set(if success { 1.0 } else { 0.0 });
    if success {
        gauge!("rim_database_sample_last_success_timestamp_seconds", "sample" => sample)
            .set(chrono::Utc::now().timestamp() as f64);
    } else {
        counter!("rim_database_sample_failures_total", "sample" => sample).increment(1);
    }
}

/// Set local account and activity gauges after a successful aggregate read.
///
/// # Errors
/// Propagates repository read errors without replacing previous values.
pub async fn sample_usage(repo: &dyn MonitoringRepository) -> RepoResult<()> {
    let stats = repo.usage_stats().await?;
    gauge!("rim_registered_accounts").set(stats.registered_accounts.max(0) as f64);
    gauge!("rim_active_users", "window" => "24h").set(stats.active_24h.max(0) as f64);
    gauge!("rim_active_users", "window" => "7d").set(stats.active_7d.max(0) as f64);
    Ok(())
}

/// Set precise pending delivery gauges after a successful aggregate read.
///
/// # Errors
/// Propagates repository read errors without replacing previous values.
pub async fn sample_delivery(repo: &dyn MonitoringRepository) -> RepoResult<()> {
    let stats = repo.delivery_stats().await?;
    for (name, value) in [
        ("rim_relay_pending_items", stats.items),
        ("rim_relay_pending_bytes", stats.bytes),
        ("rim_relay_pending_device_items", stats.device_items),
        ("rim_relay_pending_users", stats.users),
        ("rim_relay_pending_devices", stats.devices),
    ] {
        gauge!(name).set(value.max(0) as f64);
    }
    gauge!("rim_relay_oldest_pending_age_seconds").set(stats.oldest_age_seconds.max(0.0));
    Ok(())
}

#[cfg(test)]
mod tests {
    use metrics_util::debugging::{DebugValue, DebuggingRecorder};
    use rim_core::monitoring::{DeliveryStats, MockMonitoringRepository, UsageStats};
    use rim_core::repository::error::{RepoError, RepoErrorKind};

    use super::*;

    #[tokio::test]
    async fn successful_samples_set_distinct_recipient_and_device_counts() {
        let recorder = DebuggingRecorder::new();
        let snapshot = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        describe();
        let mut repo = MockMonitoringRepository::new();
        repo.expect_usage_stats().returning(|| {
            Box::pin(async {
                Ok(UsageStats {
                    registered_accounts: 3,
                    active_24h: 1,
                    active_7d: 2,
                })
            })
        });
        repo.expect_delivery_stats().returning(|| {
            Box::pin(async {
                Ok(DeliveryStats {
                    items: 2,
                    bytes: 20,
                    device_items: 3,
                    users: 1,
                    devices: 2,
                    oldest_age_seconds: 60.0,
                })
            })
        });
        sample_usage(&repo).await.unwrap();
        sample_delivery(&repo).await.unwrap();
        sample_outcome("usage", true);
        let values = snapshot.snapshot().into_vec();
        for (name, expected) in [
            ("rim_registered_accounts", 3.0),
            ("rim_relay_pending_items", 2.0),
            ("rim_relay_pending_device_items", 3.0),
            ("rim_relay_pending_devices", 2.0),
            ("rim_relay_oldest_pending_age_seconds", 60.0),
        ] {
            assert!(
                values
                    .iter()
                    .any(|(key, _, _, value)| key.key().name() == name
                        && *value == DebugValue::Gauge(expected.into())),
                "missing {name}"
            );
        }
    }

    #[tokio::test]
    async fn failure_preserves_the_previous_successful_value_and_marks_sampler_unhealthy() {
        let recorder = DebuggingRecorder::new();
        let snapshot = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        gauge!("rim_registered_accounts").set(8.0);
        let mut repo = MockMonitoringRepository::new();
        repo.expect_usage_stats()
            .returning(|| Box::pin(async { Err(RepoError::new(RepoErrorKind::Unavailable)) }));
        assert!(sample_usage(&repo).await.is_err());
        sample_outcome("usage", false);
        let values = snapshot.snapshot().into_vec();
        assert!(values.iter().any(|(key, _, _, value)| key.key().name()
            == "rim_registered_accounts"
            && *value == DebugValue::Gauge(8.0.into())));
        assert!(values.iter().any(|(key, _, _, value)| key.key().name()
            == "rim_database_sample_success"
            && *value == DebugValue::Gauge(0.0.into())));
        assert!(values.iter().any(|(key, _, _, value)| key.key().name()
            == "rim_database_sample_failures_total"
            && *value == DebugValue::Counter(1)));
    }
}
