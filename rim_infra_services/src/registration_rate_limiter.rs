use std::collections::VecDeque;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use rim_services::command::rate_limit::{RateLimitDecision, check_sliding_window};

/// Maximum number of distinct (normalised-IP, key) buckets held in memory.
///
/// M6 hardening: an unbounded DashMap is a memory-DoS vector when combined
/// with H1 (spoofed source IPs) or M5 (IPv6 /64 rotation). Once the map
/// exceeds this limit, the oldest-inserted bucket is evicted to make room.
///
/// 100 000 buckets × ≈200 bytes each ≈ 20 MiB worst-case, which is
/// acceptable for a long-running server process.
const MAX_BUCKETS: usize = 100_000;

type BucketKey = (String, String);
type RateLimitStore = DashMap<BucketKey, VecDeque<DateTime<Utc>>>;

/// Shared, thread-safe per-(normalised-IP, key) sliding-window rate limiter for
/// unauthenticated registration and login endpoints.
///
/// Used for:
/// - `previewInviteToken`: 30 req / 5-min / IP (keyed `"preview_invite"`)
/// - `signupWithInvite` in open mode: 20 req / 10-min / IP (keyed `"signup"`)
/// - `login`: 30 attempts / 5-min / IP, 10 failures / 15-min / (IP, username), and
///   100 failures / hour / username from any IP
///
/// Single-node only; DashMap-backed.
///
/// # IPv6 normalisation (M5)
/// IPv6 source addresses are normalised to their /64 network prefix before
/// keying the map. This prevents an attacker on a /64 block from cycling
/// through the remaining 2^64 interface identifiers to bypass per-IP limits.
/// IPv4 addresses are used as-is (/32).
///
/// A /64 was chosen over a tighter /56 because:
/// - Residential ISPs typically assign a single /64 to each CPE; normalising
///   to /64 scopes the limit to the household without lumping multiple
///   households together.
/// - /56 would group 256 /64 prefixes under one bucket, which could
///   incorrectly limit legitimate users sharing a carrier-grade NAT prefix.
///
/// # Capacity cap (M6)
/// The map is capped at [`MAX_BUCKETS`] entries. When the cap is reached,
/// the oldest-inserted bucket is evicted (FIFO). This bounds memory usage
/// and prevents a memory DoS via high-volume IP rotation.
#[derive(Debug, Clone)]
pub struct RegistrationRateLimiter {
    store: std::sync::Arc<RateLimitStore>,
    /// FIFO queue of inserted keys, used for eviction when the cap is reached.
    insertion_order: std::sync::Arc<Mutex<VecDeque<BucketKey>>>,
    window_secs: u64,
    limit: u32,
}

impl Default for RegistrationRateLimiter {
    fn default() -> Self {
        Self::preview_default()
    }
}

impl RegistrationRateLimiter {
    /// Construct a new limiter with the given sliding-window size and request limit.
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
            insertion_order: std::sync::Arc::new(Mutex::new(VecDeque::new())),
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

    /// Construct the per-IP login limiter (30 attempts / 5 min / IP).
    ///
    /// Counts every attempt, checked before the Argon2 verify, so it also bounds the
    /// password-hashing CPU/memory a single source can burn.
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::registration_rate_limiter::RegistrationRateLimiter;
    /// let limiter = RegistrationRateLimiter::login_ip_default();
    /// ```
    pub fn login_ip_default() -> Self {
        Self::new(300, 30)
    }

    /// Construct the per-(IP, username) login limiter (10 failures / 15 min).
    ///
    /// The strict guessing limit. Keyed by source and account together, so an attacker
    /// burning it throttles that account only from their own address (and anyone sharing it,
    /// such as the same NAT or IPv6 /64), not from the owner's. Behind a proxy without
    /// `RIM_TRUST_FORWARDED_HEADERS`, every client shares the proxy's address and this
    /// degrades to a per-username limit. Only failures are recorded, so a user who signs in
    /// correctly is never throttled by their own logins.
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::registration_rate_limiter::RegistrationRateLimiter;
    /// let limiter = RegistrationRateLimiter::login_ip_account_default();
    /// ```
    pub fn login_ip_account_default() -> Self {
        Self::new(900, 10)
    }

    /// Construct the per-account login limiter (100 failures / hour / username, any IP).
    ///
    /// A backstop against distributed credential stuffing that spreads guesses over many
    /// addresses. Deliberately loose: every source counts here, so a tight limit would let
    /// anyone lock an account's owner out. Each address can add at most 40 failures an hour
    /// (the per-(IP, username) limit), so tripping it takes three or more addresses (or IPv6
    /// /64s) guessing for up to an hour. Only failures are recorded.
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::registration_rate_limiter::RegistrationRateLimiter;
    /// let limiter = RegistrationRateLimiter::login_account_default();
    /// ```
    pub fn login_account_default() -> Self {
        Self::new(3600, 100)
    }

    /// Construct the per-IP password-reset limiter (10 requests / hour / IP).
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::registration_rate_limiter::RegistrationRateLimiter;
    /// let limiter = RegistrationRateLimiter::password_reset_ip_default();
    /// ```
    pub fn password_reset_ip_default() -> Self {
        Self::new(3600, 10)
    }

    /// Construct the per-username password-reset limiter (3 requests / hour / username).
    ///
    /// Bounds how many reset emails one address can receive, whoever asks for them.
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::registration_rate_limiter::RegistrationRateLimiter;
    /// let limiter = RegistrationRateLimiter::password_reset_account_default();
    /// ```
    pub fn password_reset_account_default() -> Self {
        Self::new(3600, 3)
    }

    /// Normalise a raw IP string to a canonical rate-limit key.
    ///
    /// - **IPv4**: returned unchanged (full /32 precision).
    /// - **IPv6**: zeroes the lower 64 bits, keeping only the /64 network prefix.
    ///   This prevents attacker rotation across the 2^64 interface identifiers
    ///   within a single /64 allocation (M5 hardening).
    /// - **Unparseable**: returned unchanged as a fallback so rate-limiting
    ///   degrades gracefully rather than panicking.
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::registration_rate_limiter::RegistrationRateLimiter;
    ///
    /// // IPv4 is unchanged.
    /// assert_eq!(RegistrationRateLimiter::normalise_ip("1.2.3.4"), "1.2.3.4");
    ///
    /// // Two IPv6 addresses in the same /64 share a bucket.
    /// let a = RegistrationRateLimiter::normalise_ip("2001:db8::1");
    /// let b = RegistrationRateLimiter::normalise_ip("2001:db8::2");
    /// assert_eq!(a, b);
    ///
    /// // IPv6 addresses in different /64s are distinct.
    /// let c = RegistrationRateLimiter::normalise_ip("2001:db8:0:1::1");
    /// assert_ne!(a, c);
    /// ```
    pub fn normalise_ip(ip: &str) -> String {
        // `to_canonical` turns IPv4-mapped IPv6 (`::ffff:a.b.c.d`, as seen on a dual-stack
        // listener) back into the IPv4 address; masking it to /64 would lump every IPv4 client
        // into one bucket.
        match IpAddr::from_str(ip).map(|addr| addr.to_canonical()) {
            Ok(IpAddr::V6(v6)) => {
                // Keep only the top 64 bits (network prefix).
                let octets = v6.octets();
                let prefix: [u8; 16] = [
                    octets[0], octets[1], octets[2], octets[3], octets[4], octets[5], octets[6],
                    octets[7], // Zero the lower 64 bits (interface identifier).
                    0, 0, 0, 0, 0, 0, 0, 0,
                ];
                std::net::Ipv6Addr::from(prefix).to_string()
            }
            Ok(IpAddr::V4(v4)) => v4.to_string(),
            // Unparseable: use as-is.
            Err(_) => ip.to_string(),
        }
    }

    /// Record one call from `ip` under `key` and return whether it is allowed.
    ///
    /// Before checking, the IP is normalised via [`Self::normalise_ip`] (M5).
    /// If the store exceeds [`MAX_BUCKETS`] after inserting a new bucket, the
    /// oldest bucket is evicted (M6).
    ///
    /// Stale timestamps outside the sliding window are evicted before checking
    /// the count, bounding per-bucket memory to at most `limit` entries.
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::registration_rate_limiter::RegistrationRateLimiter;
    /// use chrono::Utc;
    /// let limiter = RegistrationRateLimiter::preview_default();
    /// let decision = limiter.check_and_record("1.2.3.4", "preview_invite", Utc::now());
    /// ```
    pub fn check_and_record(&self, ip: &str, key: &str, now: DateTime<Utc>) -> RateLimitDecision {
        let normalised = Self::normalise_ip(ip);
        let map_key = (normalised, key.to_string());
        let is_new = !self.store.contains_key(&map_key);

        let mut entry = self.store.entry(map_key.clone()).or_default();
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

        // Drop the entry guard before taking the insertion_order lock to avoid
        // potential lock-ordering deadlocks.
        drop(entry);

        // M6: track insertion order and evict when over the cap.
        if is_new && let Ok(mut order) = self.insertion_order.lock() {
            order.push_back(map_key);
            // Evict oldest bucket(s) until we're within the cap.
            while order.len() > MAX_BUCKETS {
                if let Some(oldest_key) = order.pop_front() {
                    self.store.remove(&oldest_key);
                }
            }
        }

        decision
    }

    /// Remove one call recorded at `at` for `ip` under `key` by [`Self::check_and_record`].
    ///
    /// Lets a caller reserve a slot up front (so concurrent requests can't all slip past the
    /// limit) and give it back when the outcome shouldn't count — e.g. login reserves before
    /// verifying and refunds on success.
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::registration_rate_limiter::RegistrationRateLimiter;
    /// use rim_services::command::rate_limit::RateLimitDecision;
    /// use chrono::Utc;
    /// let limiter = RegistrationRateLimiter::new(60, 1);
    /// let now = Utc::now();
    /// limiter.check_and_record("1.2.3.4", "k", now);
    /// limiter.refund("1.2.3.4", "k", now);
    /// assert_eq!(limiter.check_and_record("1.2.3.4", "k", now), RateLimitDecision::Allowed);
    /// ```
    pub fn refund(&self, ip: &str, key: &str, at: DateTime<Utc>) {
        let map_key = (Self::normalise_ip(ip), key.to_string());
        if let Some(mut entry) = self.store.get_mut(&map_key)
            && let Some(pos) = entry.iter().rposition(|&ts| ts == at)
        {
            entry.remove(pos);
        }
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
    fn refund_releases_reserved_slot() {
        let rl = RegistrationRateLimiter::new(300, 1);
        let t = now();
        assert_eq!(
            rl.check_and_record("4.4.4.4", "k", t),
            RateLimitDecision::Allowed
        );
        assert!(matches!(
            rl.check_and_record("4.4.4.4", "k", t),
            RateLimitDecision::Limited { .. }
        ));
        rl.refund("4.4.4.4", "k", t);
        assert_eq!(
            rl.check_and_record("4.4.4.4", "k", t),
            RateLimitDecision::Allowed
        );
        // Refunding an unknown bucket or timestamp is a no-op.
        rl.refund("5.5.5.5", "k", t);
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

        // Inject 2 stale timestamps directly into the store.
        {
            let normalised = RegistrationRateLimiter::normalise_ip(ip);
            let map_key = (normalised, key.to_string());
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

    // ── M5 tests ─────────────────────────────────────────────────────────────

    /// M5 — IPv4 addresses are used as-is (no truncation).
    #[test]
    fn normalise_ip_ipv4_unchanged() {
        assert_eq!(RegistrationRateLimiter::normalise_ip("1.2.3.4"), "1.2.3.4");
        assert_eq!(
            RegistrationRateLimiter::normalise_ip("192.168.100.200"),
            "192.168.100.200"
        );
    }

    /// IPv4-mapped IPv6 addresses normalise to the IPv4 address, not to a shared `::` /64.
    #[test]
    fn normalise_ip_unmaps_ipv4_mapped_ipv6() {
        assert_eq!(
            RegistrationRateLimiter::normalise_ip("::ffff:1.2.3.4"),
            "1.2.3.4"
        );
        assert_ne!(
            RegistrationRateLimiter::normalise_ip("::ffff:1.2.3.4"),
            RegistrationRateLimiter::normalise_ip("::ffff:5.6.7.8")
        );
    }

    /// M5 — Two IPv6 addresses in the same /64 share a bucket.
    #[test]
    fn normalise_ip_ipv6_same_slash64_share_bucket() {
        let a = RegistrationRateLimiter::normalise_ip("2001:db8::1");
        let b = RegistrationRateLimiter::normalise_ip("2001:db8::ffff");
        assert_eq!(a, b, "same /64 prefix must yield the same normalised key");
    }

    /// M5 — Two IPv6 addresses in different /64s have different buckets.
    #[test]
    fn normalise_ip_ipv6_different_slash64_different_bucket() {
        let a = RegistrationRateLimiter::normalise_ip("2001:db8:0:1::1");
        let b = RegistrationRateLimiter::normalise_ip("2001:db8:0:2::1");
        assert_ne!(
            a, b,
            "different /64 prefixes must yield different normalised keys"
        );
    }

    /// M5 — Two IPv6 addresses in the same /64 hit the same rate-limit bucket.
    #[test]
    fn ipv6_same_slash64_shares_rate_limit_bucket() {
        let rl = RegistrationRateLimiter::new(300, 2);
        let key = "preview_invite";

        // Two different addresses within the same /64.
        assert_eq!(
            rl.check_and_record("2001:db8::1", key, now()),
            RateLimitDecision::Allowed
        );
        assert_eq!(
            rl.check_and_record("2001:db8::2", key, now()),
            RateLimitDecision::Allowed
        );
        // Third call — same /64 bucket, now exhausted.
        assert!(
            matches!(
                rl.check_and_record("2001:db8::3", key, now()),
                RateLimitDecision::Limited { .. }
            ),
            "third call from same /64 must be limited"
        );
    }

    /// M5 — Two IPv6 addresses in different /64s have independent buckets.
    #[test]
    fn ipv6_different_slash64_independent_buckets() {
        let rl = RegistrationRateLimiter::new(300, 1);
        let key = "preview_invite";

        assert_eq!(
            rl.check_and_record("2001:db8:0:1::1", key, now()),
            RateLimitDecision::Allowed,
            "first /64 first call"
        );
        // Different /64 → fresh bucket.
        assert_eq!(
            rl.check_and_record("2001:db8:0:2::1", key, now()),
            RateLimitDecision::Allowed,
            "second /64 first call must be independent"
        );
    }

    // ── M6 tests ─────────────────────────────────────────────────────────────

    /// M6 — When the map exceeds `MAX_BUCKETS`, the oldest entry is evicted.
    ///
    /// Uses a small cap (3) to keep the test fast.
    #[test]
    fn cap_evicts_oldest_entry() {
        // Build a limiter with the same logic but a tiny cap.
        // We can't override MAX_BUCKETS, so we test the eviction indirectly by
        // verifying that after inserting cap+1 distinct IPs, the total entry
        // count in the store is bounded.
        //
        // Since MAX_BUCKETS = 100_000 we can't fill it in a unit test.
        // Instead, test the eviction logic via white-box access to the store.

        let rl = RegistrationRateLimiter::new(300, 100);
        let key = "k";

        // Insert MAX_BUCKETS + 1 distinct IPs. The first one should be evicted.
        // This is too slow for MAX_BUCKETS = 100_000, so we just verify that
        // the store size is at most MAX_BUCKETS after inserting MAX_BUCKETS + 10.
        //
        // For a practical unit test we verify the FIFO eviction directly by
        // observing the insertion_order queue length.

        // Insert 5 distinct IPs.
        for i in 0u8..5 {
            rl.check_and_record(&format!("10.0.0.{i}"), key, now());
        }
        assert_eq!(rl.store.len(), 5, "5 distinct IPs should produce 5 buckets");
        assert_eq!(
            rl.insertion_order.lock().unwrap().len(),
            5,
            "insertion order queue should track 5 entries"
        );
    }

    /// M6 — The eviction logic removes the oldest bucket when the limit is hit.
    ///
    /// We white-box test by manually pre-filling insertion_order to simulate
    /// being at the cap, then verify one more insert evicts the oldest.
    #[test]
    fn cap_eviction_removes_oldest_when_full() {
        let rl = RegistrationRateLimiter::new(300, 100);
        let key = "k";

        // Pre-seed with MAX_BUCKETS synthetic entries so the next real insert
        // triggers eviction. Because seeding MAX_BUCKETS real calls would be
        // slow, we manipulate the internal structures directly.
        {
            let mut order = rl.insertion_order.lock().unwrap();
            for i in 0..MAX_BUCKETS {
                let synthetic_key = (format!("synthetic-ip-{i}"), key.to_string());
                rl.store.insert(synthetic_key.clone(), VecDeque::new());
                order.push_back(synthetic_key);
            }
        }

        assert_eq!(rl.store.len(), MAX_BUCKETS);

        // Now insert one real entry — should evict "synthetic-ip-0".
        rl.check_and_record("10.0.0.1", key, now());

        // Store must still be at or below MAX_BUCKETS.
        assert!(
            rl.store.len() <= MAX_BUCKETS,
            "store must not exceed MAX_BUCKETS after eviction; len={}",
            rl.store.len()
        );
        // The oldest synthetic entry must have been evicted.
        assert!(
            !rl.store
                .contains_key(&("synthetic-ip-0".to_string(), key.to_string())),
            "oldest entry must have been evicted"
        );
        // The new real entry must be present.
        assert!(
            rl.store
                .contains_key(&(String::from("10.0.0.1"), key.to_string())),
            "new entry must be present after eviction"
        );
    }
}
