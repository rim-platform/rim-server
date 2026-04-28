/// Newtype wrapper for the preview-invite rate limiter used as GraphQL context data.
///
/// Keyed `("preview_invite", ip)`, 30 req / 5 min / IP (§4.7).
#[derive(Clone, Debug)]
pub struct PreviewInviteRateLimiter(pub RegistrationRateLimiter);

impl PreviewInviteRateLimiter {
    /// Construct the default preview rate limiter.
    pub fn new() -> Self {
        Self(RegistrationRateLimiter::preview_default())
    }
}

impl Default for PreviewInviteRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Newtype wrapper for the open-mode signup rate limiter used as GraphQL context data.
///
/// Required when `RIM_OPEN_REGISTRATION=true`; boot check fails if absent.
#[derive(Clone, Debug)]
pub struct SignupRateLimiter(pub RegistrationRateLimiter);

impl SignupRateLimiter {
    /// Construct the default signup rate limiter.
    pub fn new() -> Self {
        Self(RegistrationRateLimiter::signup_default())
    }
}

impl Default for SignupRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use rim_services::command::rate_limit::{RateLimitDecision, check_sliding_window};

type RegistrationRateLimitStore = DashMap<(String, String), VecDeque<DateTime<Utc>>>;

/// Shared, thread-safe per-(IP, key) sliding-window rate limiter for
/// unauthenticated registration endpoints.
///
/// Used for:
/// - `previewInviteToken`: 30 req / 5-min / IP (keyed `"preview_invite"`)
/// - `signupWithInvite` in open mode: configurable via `SIGNUP_LIMIT` / `SIGNUP_WINDOW_SECS`
///
/// Single-node only; DashMap-backed.
#[derive(Debug, Default, Clone)]
pub struct RegistrationRateLimiter {
    store: std::sync::Arc<RegistrationRateLimitStore>,
    window_secs: u64,
    limit: u32,
}

impl RegistrationRateLimiter {
    /// Construct a new limiter with the given window and limit.
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::registration_rate_limiter::RegistrationRateLimiter;
    /// // 30 requests per 5-minute window per IP
    /// let limiter = RegistrationRateLimiter::new(300, 30);
    /// ```
    pub fn new(window_secs: u64, limit: u32) -> Self {
        Self {
            store: std::sync::Arc::new(DashMap::new()),
            window_secs,
            limit,
        }
    }

    /// Construct the standard preview rate limiter (30 req / 5 min / IP).
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::registration_rate_limiter::RegistrationRateLimiter;
    /// let limiter = RegistrationRateLimiter::preview_default();
    /// ```
    pub fn preview_default() -> Self {
        Self::new(300, 30)
    }

    /// Construct the standard open-mode signup rate limiter (20 req / 10 min / IP).
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::registration_rate_limiter::RegistrationRateLimiter;
    /// let limiter = RegistrationRateLimiter::signup_default();
    /// ```
    pub fn signup_default() -> Self {
        // 20 signups per 10-minute window per IP — tighter than preview but
        // less tight than auth attempts.
        Self::new(600, 20)
    }

    /// Record one call from `ip` under `key` and return whether it is allowed.
    ///
    /// Evicts timestamps outside the window before checking, bounding memory
    /// to `limit` entries per (ip, key) pair.
    ///
    /// # Errors
    /// This function is infallible; always returns a [`RateLimitDecision`].
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::registration_rate_limiter::RegistrationRateLimiter;
    /// use chrono::Utc;
    /// let limiter = RegistrationRateLimiter::preview_default();
    /// let decision = limiter.check_and_record("1.2.3.4", "preview_invite", Utc::now());
    /// ```
    pub fn check_and_record(&self, ip: &str, key: &str, now: DateTime<Utc>) -> RateLimitDecision {
        let map_key = (ip.to_string(), key.to_string());
        let mut entry = self.store.entry(map_key).or_default();
        let timestamps = entry.value_mut();

        // Evict stale entries.
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
    fn preview_default_allows_30_rejects_31st() {
        let rl = RegistrationRateLimiter::preview_default();
        let ip = "10.0.0.1";
        let key = "preview_invite";

        for i in 1..=30 {
            assert_eq!(
                rl.check_and_record(ip, key, now()),
                RateLimitDecision::Allowed,
                "call {i} should be allowed"
            );
        }

        assert!(
            matches!(
                rl.check_and_record(ip, key, now()),
                RateLimitDecision::Limited { .. }
            ),
            "31st call must be limited"
        );
    }

    #[test]
    fn different_ips_are_isolated() {
        let rl = RegistrationRateLimiter::new(300, 2);
        let key = "preview_invite";

        rl.check_and_record("1.1.1.1", key, now());
        rl.check_and_record("1.1.1.1", key, now());
        assert!(matches!(
            rl.check_and_record("1.1.1.1", key, now()),
            RateLimitDecision::Limited { .. }
        ));

        assert_eq!(
            rl.check_and_record("2.2.2.2", key, now()),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn stale_entries_evicted_before_check() {
        let rl = RegistrationRateLimiter::new(300, 2);
        let ip = "3.3.3.3";
        let key = "preview_invite";

        // Inject 2 stale timestamps.
        {
            let map_key = (ip.to_string(), key.to_string());
            let mut entry = rl.store.entry(map_key).or_default();
            let stale = Utc::now() - chrono::Duration::seconds(310);
            entry.push_back(stale);
            entry.push_back(stale);
        }

        // Both stale, so first real call should be allowed.
        assert_eq!(
            rl.check_and_record(ip, key, now()),
            RateLimitDecision::Allowed
        );
    }
}
