use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use rim_core::user::model::UserId;
use rim_services::command::rate_limit::{RateLimitDecision, check_sliding_window};

const WINDOW_SECS: u64 = 60;

type RateLimitStore = DashMap<(UserId, String), VecDeque<DateTime<Utc>>>;

/// Shared, thread-safe per-(UserId, command_id) sliding-window rate limiter.
///
/// Single-node only — multi-node coordination is deferred post-Phase-5.
/// The store is `Clone` so `DefaultCommandService` can hold an `Arc<RateLimiter>`.
#[derive(Debug, Default, Clone)]
pub(crate) struct RateLimiter {
    store: std::sync::Arc<RateLimitStore>,
}

impl RateLimiter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Record one call and return whether it is allowed under `limit`.
    /// Evicts timestamps older than the window before the check, keeping memory
    /// bounded to `limit` entries per key.
    pub(crate) fn check_and_record(
        &self,
        actor: UserId,
        command_id: &str,
        limit: u32,
        now: DateTime<Utc>,
    ) -> RateLimitDecision {
        let key = (actor, command_id.to_string());
        let mut entry = self.store.entry(key).or_default();
        let timestamps = entry.value_mut();

        // Evict stale entries.
        while let Some(&oldest) = timestamps.front() {
            if now.signed_duration_since(oldest).num_seconds() >= WINDOW_SECS as i64 {
                timestamps.pop_front();
            } else {
                break;
            }
        }

        let window_slice: Vec<DateTime<Utc>> = timestamps.iter().copied().collect();
        let decision = check_sliding_window(&window_slice, now, WINDOW_SECS, limit);

        if decision == RateLimitDecision::Allowed {
            timestamps.push_back(now);
        }

        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::user::model::UserId;

    fn uid(n: u128) -> UserId {
        UserId::from(uuid::Uuid::from_u128(n))
    }

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    fn past(secs: i64) -> DateTime<Utc> {
        Utc::now() - chrono::Duration::seconds(secs)
    }

    #[test]
    fn five_calls_allowed_sixth_limited() {
        let rl = RateLimiter::new();
        let actor = uid(1);
        let cmd = "chat.post_message";
        let limit = 5;

        for i in 1..=5 {
            assert_eq!(
                rl.check_and_record(actor, cmd, limit, now()),
                RateLimitDecision::Allowed,
                "call {i} should be allowed"
            );
        }

        match rl.check_and_record(actor, cmd, limit, now()) {
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => {
                assert!(retry_after_seconds > 0);
            }
            RateLimitDecision::Allowed => panic!("6th call must be limited"),
        }
    }

    #[test]
    fn stale_entries_evicted_before_check() {
        let rl = RateLimiter::new();
        let actor = uid(2);
        let cmd = "chat.post_message";
        let limit = 3;

        // Manually inject 3 stale timestamps (> 60s ago).
        {
            let key = (actor, cmd.to_string());
            let mut entry = rl.store.entry(key).or_default();
            let ts = past(65);
            entry.push_back(ts);
            entry.push_back(ts);
            entry.push_back(ts);
        }

        // Even though 3 were pre-filled, they're stale → first real call allowed.
        assert_eq!(
            rl.check_and_record(actor, cmd, limit, now()),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn actor_isolation() {
        let rl = RateLimiter::new();
        let alice = uid(3);
        let bob = uid(4);
        let cmd = "chat.post_message";
        let limit = 2;

        // Alice fills her limit.
        rl.check_and_record(alice, cmd, limit, now());
        rl.check_and_record(alice, cmd, limit, now());
        assert!(matches!(
            rl.check_and_record(alice, cmd, limit, now()),
            RateLimitDecision::Limited { .. }
        ));

        // Bob is unaffected.
        assert_eq!(
            rl.check_and_record(bob, cmd, limit, now()),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn command_isolation() {
        let rl = RateLimiter::new();
        let actor = uid(5);
        let limit = 2;

        rl.check_and_record(actor, "chat.send", limit, now());
        rl.check_and_record(actor, "chat.send", limit, now());
        assert!(matches!(
            rl.check_and_record(actor, "chat.send", limit, now()),
            RateLimitDecision::Limited { .. }
        ));

        // Different command unaffected.
        assert_eq!(
            rl.check_and_record(actor, "chat.read", limit, now()),
            RateLimitDecision::Allowed
        );
    }
}
