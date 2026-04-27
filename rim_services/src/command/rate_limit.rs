use chrono::{DateTime, Utc};

/// Outcome of a rate-limit check for a single call slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateLimitDecision {
    Allowed,
    /// Caller has exhausted the window. `retry_after_seconds` is the number of
    /// seconds until the oldest token in the current window expires and one
    /// slot opens.
    Limited {
        retry_after_seconds: u64,
    },
}

/// Pure sliding-window arithmetic — no state. Given a sorted list of prior
/// call timestamps within the current window (most-recent first is fine; the
/// function sorts internally), the current time, the window length (seconds),
/// and the maximum calls allowed, returns whether one more call is allowed.
///
/// `window_timestamps` contains every call made in the *last `window_secs`
/// seconds* (stale entries must have been evicted before calling). The caller
/// should pass `limit` entries at most (extras are ignored).
pub fn check_sliding_window(
    window_timestamps: &[DateTime<Utc>],
    now: DateTime<Utc>,
    window_secs: u64,
    limit: u32,
) -> RateLimitDecision {
    let count = window_timestamps
        .iter()
        .filter(|&&ts| {
            let age = now.signed_duration_since(ts).num_seconds();
            age >= 0 && age < window_secs as i64
        })
        .count();

    if count < limit as usize {
        return RateLimitDecision::Allowed;
    }

    // Find the oldest timestamp still in window — when it expires a slot opens.
    let oldest_in_window = window_timestamps
        .iter()
        .filter(|&&ts| {
            let age = now.signed_duration_since(ts).num_seconds();
            age >= 0 && age < window_secs as i64
        })
        .min()
        .copied();

    let retry_after_seconds = oldest_in_window
        .map(|ts| {
            let age_secs = now.signed_duration_since(ts).num_seconds().max(0) as u64;
            window_secs.saturating_sub(age_secs) + 1
        })
        .unwrap_or(1);

    RateLimitDecision::Limited {
        retry_after_seconds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(secs_ago: i64) -> DateTime<Utc> {
        Utc::now() - chrono::Duration::seconds(secs_ago)
    }

    #[test]
    fn under_limit_is_allowed() {
        let calls: Vec<_> = (0..4).map(|i| ts(i * 5)).collect();
        assert_eq!(
            check_sliding_window(&calls, Utc::now(), 60, 5),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn at_limit_is_limited() {
        let calls: Vec<_> = (0..5).map(|i| ts(i * 5)).collect();
        match check_sliding_window(&calls, Utc::now(), 60, 5) {
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => {
                assert!(retry_after_seconds > 0);
            }
            RateLimitDecision::Allowed => panic!("expected Limited"),
        }
    }

    #[test]
    fn stale_calls_outside_window_do_not_count() {
        // 4 calls > 60s ago (stale), 4 fresh calls — still under limit of 5
        let mut calls: Vec<_> = (0..4).map(|i| ts(65 + i * 5)).collect();
        calls.extend((0..4).map(|i| ts(i * 5)));
        assert_eq!(
            check_sliding_window(&calls, Utc::now(), 60, 5),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn unlimited_none_rate_limit_always_allowed() {
        // Callers with `rate_limit_per_minute: None` never call into this fn.
        // This test verifies that an arbitrarily high limit never fires.
        let calls: Vec<_> = (0..100).map(|i| ts(i % 59)).collect();
        assert_eq!(
            check_sliding_window(&calls, Utc::now(), 60, u32::MAX),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn retry_after_is_positive_and_at_most_window() {
        let calls: Vec<_> = (0..5).map(|i| ts(i * 10)).collect();
        let window = 60u64;
        match check_sliding_window(&calls, Utc::now(), window, 5) {
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => {
                assert!(retry_after_seconds >= 1);
                assert!(retry_after_seconds <= window + 1);
            }
            RateLimitDecision::Allowed => panic!("expected Limited"),
        }
    }
}
