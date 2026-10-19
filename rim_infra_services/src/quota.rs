//! What each local user of a relay may hold and send, and the fixed-window counter the daily
//! quotas use.

use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use rim_services::command::rate_limit::RateLimitDecision;

/// Most keys a [`DailyCounter`] holds before it forgets windows that ended.
const MAX_KEYS: usize = 100_000;

/// Per-user quotas. The queued-bytes cap lives with the relay's queue settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quotas {
    /// Largest profile card payload, in bytes (at most 128 KiB, the statement cap).
    pub card_max_bytes: usize,
    /// Unclaimed one-time key packages per device and cipher suite.
    pub key_packages_per_device: i64,
    /// Devices that are not revoked, per user.
    pub devices_per_user: usize,
    /// Requests a device sends through `/api/remote/…` per minute.
    pub forwards_per_device_per_min: u32,
    /// Requests a user sends through `/api/remote/…` per day.
    pub forwards_per_user_per_day: u32,
    /// DM requests: those one sender principal files on this relay per day, and invites a
    /// local user sends to people on other servers they do not know per day.
    pub dm_requests_per_day: u32,
    /// Messages a user sends per minute, with their burst; `None` sets no limit.
    pub messages: Option<crate::token_bucket::BucketRate>,
    /// Messages an account in probation sends per minute, instead of `messages`; `None` keeps
    /// it on `messages`.
    pub probation_messages: Option<crate::token_bucket::BucketRate>,
}

/// Largest card the statement rules allow.
pub const MAX_CARD_BYTES: usize = 128 * 1024;

impl Default for Quotas {
    fn default() -> Self {
        Self {
            card_max_bytes: MAX_CARD_BYTES,
            key_packages_per_device: rim_services::e2ee::MAX_AVAILABLE_KEY_PACKAGES,
            devices_per_user: 32,
            forwards_per_device_per_min: 120,
            forwards_per_user_per_day: 50_000,
            dm_requests_per_day: 20,
            messages: None,
            probation_messages: None,
        }
    }
}

/// Counts per key in windows of a fixed length starting at a key's first use. Unlike a sliding
/// window it keeps one count per key, so a large daily limit costs no memory. In memory, per
/// node.
#[derive(Debug)]
pub struct DailyCounter {
    counts: DashMap<String, (DateTime<Utc>, u64)>,
    window: Duration,
    limit: u64,
}

impl DailyCounter {
    /// A counter of `limit` per day.
    pub fn per_day(limit: u64) -> Self {
        Self::new(Duration::days(1), limit)
    }

    pub fn new(window: Duration, limit: u64) -> Self {
        Self {
            counts: DashMap::new(),
            window,
            limit,
        }
    }

    /// Forget windows that ended once the counter holds `MAX_KEYS` keys.
    fn trim(&self, now: DateTime<Utc>) {
        if self.counts.len() >= MAX_KEYS {
            let window = self.window;
            self.counts.retain(|_, (start, _)| now - *start < window);
        }
    }

    /// Count one use of `key` at `now` unless the key used its limit in the current window.
    pub fn check_and_record(&self, key: &str, now: DateTime<Utc>) -> RateLimitDecision {
        self.trim(now);
        let mut entry = self.counts.entry(key.to_string()).or_insert((now, 0));
        let (start, count) = entry.value_mut();
        if now - *start >= self.window {
            *start = now;
            *count = 0;
        }
        if *count >= self.limit {
            let retry = (*start + self.window - now).num_seconds().max(1);
            return RateLimitDecision::Limited {
                retry_after_seconds: u64::try_from(retry).unwrap_or(1),
            };
        }
        *count += 1;
        RateLimitDecision::Allowed
    }

    /// What `key` may still count in its window at `now`: the whole limit once that window
    /// ended or before the key's first use.
    pub fn remaining(&self, key: &str, now: DateTime<Utc>) -> u64 {
        match self.counts.get(key) {
            Some(entry) if now - entry.0 < self.window => self.limit.saturating_sub(entry.1),
            _ => self.limit,
        }
    }

    /// Count `amount` for `key` at `now`, starting a window when the last one ended.
    pub fn add(&self, key: &str, now: DateTime<Utc>, amount: u64) {
        self.trim(now);
        let mut entry = self.counts.entry(key.to_string()).or_insert((now, 0));
        let (start, count) = entry.value_mut();
        if now - *start >= self.window {
            *start = now;
            *count = 0;
        }
        *count = count.saturating_add(amount);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_public_relay_quotas() {
        let q = Quotas::default();
        assert_eq!(q.card_max_bytes, 131_072);
        assert_eq!(q.key_packages_per_device, 100);
        assert_eq!(q.devices_per_user, 32);
        assert_eq!(q.forwards_per_device_per_min, 120);
        assert_eq!(q.forwards_per_user_per_day, 50_000);
        assert_eq!(q.dm_requests_per_day, 20);
        assert_eq!(q.messages, None);
        assert_eq!(q.probation_messages, None);
    }

    #[test]
    fn a_daily_counter_refuses_past_its_limit_until_the_window_ends() {
        let counter = DailyCounter::per_day(2);
        let t0 = Utc::now();
        assert_eq!(
            counter.check_and_record("a", t0),
            RateLimitDecision::Allowed
        );
        assert_eq!(
            counter.check_and_record("a", t0 + Duration::hours(1)),
            RateLimitDecision::Allowed
        );
        match counter.check_and_record("a", t0 + Duration::hours(2)) {
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => assert_eq!(retry_after_seconds, 22 * 3600),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(
            counter.check_and_record("b", t0),
            RateLimitDecision::Allowed
        );
        assert_eq!(
            counter.check_and_record("a", t0 + Duration::days(1)),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn a_daily_counter_adds_amounts_and_tells_what_remains_until_the_window_ends() {
        let counter = DailyCounter::per_day(100);
        let t0 = Utc::now();
        assert_eq!(counter.remaining("a", t0), 100);
        counter.add("a", t0, 60);
        counter.add("a", t0 + Duration::hours(1), 30);
        assert_eq!(counter.remaining("a", t0 + Duration::hours(2)), 10);
        counter.add("a", t0 + Duration::hours(3), 30);
        assert_eq!(counter.remaining("a", t0 + Duration::hours(3)), 0);
        assert_eq!(counter.remaining("b", t0), 100);
        assert_eq!(counter.remaining("a", t0 + Duration::days(1)), 100);
        counter.add("a", t0 + Duration::days(1), 5);
        assert_eq!(counter.remaining("a", t0 + Duration::days(1)), 95);
    }
}
