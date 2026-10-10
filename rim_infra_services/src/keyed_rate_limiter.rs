use std::collections::VecDeque;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use rim_services::command::rate_limit::{RateLimitDecision, check_sliding_window};

/// Maximum number of distinct key buckets held in memory.
///
/// Mirrors the cap on [`crate::registration_rate_limiter::RegistrationRateLimiter`]: an
/// unbounded map is a memory-DoS vector. Once the map exceeds this limit, the
/// oldest-inserted bucket is evicted to make room.
const MAX_BUCKETS: usize = 100_000;

type RateLimitStore = DashMap<String, VecDeque<DateTime<Utc>>>;

/// Shared, thread-safe sliding-window rate limiter keyed by an arbitrary string, such as a
/// peer server id.
///
/// Unlike [`crate::registration_rate_limiter::RegistrationRateLimiter`], the key is chosen by
/// the caller rather than being the caller's IP.
///
/// Single-node only; DashMap-backed.
#[derive(Debug, Clone)]
pub struct KeyedRateLimiter {
    store: std::sync::Arc<RateLimitStore>,
    /// FIFO queue of inserted keys, used for eviction when the cap is reached.
    insertion_order: std::sync::Arc<Mutex<VecDeque<String>>>,
    window_secs: u64,
    limit: u32,
}

impl KeyedRateLimiter {
    /// Construct a new limiter with the given sliding-window size and per-key request limit.
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::keyed_rate_limiter::KeyedRateLimiter;
    /// // 60 requests per 60-second window per key
    /// let limiter = KeyedRateLimiter::new(60, 60);
    /// ```
    pub fn new(window_secs: u64, limit: u32) -> Self {
        Self {
            store: std::sync::Arc::new(DashMap::new()),
            insertion_order: std::sync::Arc::new(Mutex::new(VecDeque::new())),
            window_secs,
            limit,
        }
    }

    /// Record one call for `key` and return whether it is allowed.
    ///
    /// Stale timestamps outside the sliding window are evicted before checking the count,
    /// bounding per-bucket memory to at most `limit` entries. If the store exceeds
    /// [`MAX_BUCKETS`] after inserting a new bucket, the oldest bucket is evicted.
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::keyed_rate_limiter::KeyedRateLimiter;
    /// use chrono::Utc;
    /// let limiter = KeyedRateLimiter::new(60, 60);
    /// let decision = limiter.check_and_record("some-key", Utc::now());
    /// ```
    pub fn check_and_record(&self, key: &str, now: DateTime<Utc>) -> RateLimitDecision {
        let map_key = key.to_string();
        let is_new = !self.store.contains_key(&map_key);

        let mut entry = self.store.entry(map_key.clone()).or_default();
        let timestamps = entry.value_mut();

        while let Some(&oldest) = timestamps.front() {
            if now.signed_duration_since(oldest).num_seconds() >= self.window_secs as i64 {
                timestamps.pop_front();
            } else {
                break;
            }
        }

        let window_slice: Vec<DateTime<Utc>> = timestamps.iter().copied().collect();
        let decision = check_sliding_window(&window_slice, now, self.window_secs, self.limit);

        if decision == RateLimitDecision::Allowed {
            timestamps.push_back(now);
        }

        // Drop the entry guard before taking the insertion_order lock to avoid potential
        // lock-ordering deadlocks.
        drop(entry);

        if is_new && let Ok(mut order) = self.insertion_order.lock() {
            order.push_back(map_key);
            while order.len() > MAX_BUCKETS {
                if let Some(oldest_key) = order.pop_front() {
                    self.store.remove(&oldest_key);
                }
            }
        }

        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn allows_up_to_limit_then_blocks() {
        let rl = KeyedRateLimiter::new(60, 3);
        let token = "tok-1";

        for i in 1..=3 {
            assert_eq!(
                rl.check_and_record(token, now()),
                RateLimitDecision::Allowed,
                "call {i} should be allowed"
            );
        }

        assert!(
            matches!(
                rl.check_and_record(token, now()),
                RateLimitDecision::Limited { .. }
            ),
            "4th call must be limited"
        );
    }

    #[test]
    fn different_keys_are_isolated() {
        let rl = KeyedRateLimiter::new(60, 1);

        assert_eq!(
            rl.check_and_record("tok-a", now()),
            RateLimitDecision::Allowed
        );
        assert!(matches!(
            rl.check_and_record("tok-a", now()),
            RateLimitDecision::Limited { .. }
        ));

        // A different key has its own bucket.
        assert_eq!(
            rl.check_and_record("tok-b", now()),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn stale_entries_evicted_before_check() {
        let rl = KeyedRateLimiter::new(60, 1);
        let token = "tok-stale";

        {
            let map_key = token.to_string();
            let mut entry = rl.store.entry(map_key).or_default();
            let stale = Utc::now() - chrono::Duration::seconds(61);
            entry.push_back(stale);
        }

        // The only recorded timestamp is stale, so this call should be allowed.
        assert_eq!(
            rl.check_and_record(token, now()),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn cap_evicts_oldest_entry_when_full() {
        let rl = KeyedRateLimiter::new(60, 100);

        {
            let mut order = rl.insertion_order.lock().unwrap();
            for i in 0..MAX_BUCKETS {
                let synthetic_key = format!("synthetic-tok-{i}");
                rl.store.insert(synthetic_key.clone(), VecDeque::new());
                order.push_back(synthetic_key);
            }
        }

        assert_eq!(rl.store.len(), MAX_BUCKETS);

        rl.check_and_record("real-tok", now());

        assert!(
            rl.store.len() <= MAX_BUCKETS,
            "store must not exceed MAX_BUCKETS after eviction; len={}",
            rl.store.len()
        );
        assert!(
            !rl.store.contains_key("synthetic-tok-0"),
            "oldest entry must have been evicted"
        );
        assert!(
            rl.store.contains_key("real-tok"),
            "new entry must be present after eviction"
        );
    }
}
