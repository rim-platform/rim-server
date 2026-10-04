use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use tracing::warn;

use crate::registration_rate_limiter::RegistrationRateLimiter;

/// Maximum number of accounts tracked. Past it, the account first inserted is dropped (FIFO),
/// so filling the map takes successful logins to this many distinct accounts. At up to
/// [`MAX_SOURCES_PER_ACCOUNT`] short entries each, that is roughly 100 MB in the worst case.
const MAX_ACCOUNTS: usize = 100_000;

/// Maximum number of source addresses remembered per account; the least recently used is
/// dropped first, so one account's logins can't grow the map without bound.
const MAX_SOURCES_PER_ACCOUNT: usize = 10;

/// Source addresses that recently signed in to an account successfully.
///
/// Login lets such a source skip the per-username failure backstop, so an attacker guessing
/// from elsewhere can't lock the owner out of their usual address. Entries only come from
/// successful logins and are keyed by the exact username the login resolved (usernames are
/// case-sensitive), so an attacker can't add one for an account without its password. A
/// password reset forgets the account's sources. In memory only: a restart forgets every
/// source, which just falls back to the backstop.
///
/// Addresses are normalised like the rate limiter's (IPv6 to its /64), so everyone sharing a
/// NAT or /64 with the owner shares their exemption. Behind a proxy without
/// `RIM_TRUST_FORWARDED_HEADERS`, every client is the proxy's address, so the owner's first
/// login exempts everyone for that account; there the per-(IP, username) limit already acts
/// as a stricter per-username limit, so nothing is lost. Requests whose address can't be
/// determined share the `"unknown"` source the same way.
#[derive(Debug, Clone)]
pub(crate) struct KnownLoginSources {
    inner: Arc<Mutex<Inner>>,
    ttl: Duration,
}

#[derive(Debug, Default)]
struct Inner {
    /// Account key → (normalised source, last successful login), most recent last.
    accounts: HashMap<String, VecDeque<(String, DateTime<Utc>)>>,
    /// Account keys in insertion order, for FIFO eviction past [`MAX_ACCOUNTS`].
    order: VecDeque<String>,
}

impl KnownLoginSources {
    /// Remember sources for `ttl` after their last successful login.
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            ttl,
        }
    }

    /// The default: sources are remembered for 30 days.
    pub(crate) fn login_default() -> Self {
        Self::new(Duration::days(30))
    }

    /// Whether `ip` signed in to `account` (an exact username) successfully within the TTL
    /// before `now`. Fails closed (`false`) if the lock is poisoned.
    pub(crate) fn is_known(&self, ip: &str, account: &str, now: DateTime<Utc>) -> bool {
        let source = RegistrationRateLimiter::normalise_ip(ip);
        let Ok(inner) = self.inner.lock() else {
            warn!("known login sources lock poisoned; every source treated as unknown");
            return false;
        };
        inner.accounts.get(account).is_some_and(|sources| {
            sources
                .iter()
                .any(|(s, at)| *s == source && now.signed_duration_since(*at) < self.ttl)
        })
    }

    /// Record a successful login to `account` (an exact username) from `ip` at `now`.
    pub(crate) fn record(&self, ip: &str, account: &str, now: DateTime<Utc>) {
        let source = RegistrationRateLimiter::normalise_ip(ip);
        let Ok(mut inner) = self.inner.lock() else {
            warn!("known login sources lock poisoned; not recording login source");
            return;
        };
        let is_new = !inner.accounts.contains_key(account);
        let sources = inner.accounts.entry(account.to_string()).or_default();
        sources.retain(|(s, at)| *s != source && now.signed_duration_since(*at) < self.ttl);
        sources.push_back((source, now));
        while sources.len() > MAX_SOURCES_PER_ACCOUNT {
            sources.pop_front();
        }

        if is_new {
            inner.order.push_back(account.to_string());
            while inner.order.len() > MAX_ACCOUNTS {
                if let Some(oldest) = inner.order.pop_front() {
                    inner.accounts.remove(&oldest);
                }
            }
        }
    }

    /// Forget every source for `account` (an exact username), e.g. after a password reset.
    pub(crate) fn forget(&self, account: &str) {
        let Ok(mut inner) = self.inner.lock() else {
            warn!("known login sources lock poisoned; not forgetting account sources");
            return;
        };
        if inner.accounts.remove(account).is_some() {
            inner.order.retain(|a| a != account);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_expires_sources_per_account() {
        let known = KnownLoginSources::new(Duration::days(30));
        let t = Utc::now();
        assert!(!known.is_known("1.1.1.1", "alice", t));

        known.record("1.1.1.1", "alice", t);
        assert!(known.is_known("1.1.1.1", "alice", t));
        assert!(!known.is_known("2.2.2.2", "alice", t), "other address");
        assert!(!known.is_known("1.1.1.1", "bob", t), "other account");
        assert!(!known.is_known("1.1.1.1", "alice", t + Duration::days(30)));

        // A new login refreshes the source's age.
        known.record("1.1.1.1", "alice", t + Duration::days(20));
        assert!(known.is_known("1.1.1.1", "alice", t + Duration::days(45)));
    }

    #[test]
    fn ipv6_sources_are_grouped_by_slash64() {
        let known = KnownLoginSources::new(Duration::days(30));
        let t = Utc::now();
        known.record("2001:db8::1", "alice", t);
        assert!(known.is_known("2001:db8::ffff", "alice", t));
        assert!(!known.is_known("2001:db8:0:1::1", "alice", t));
    }

    #[test]
    fn keeps_only_the_most_recent_sources_per_account() {
        let known = KnownLoginSources::new(Duration::days(30));
        let t = Utc::now();
        for i in 0..=MAX_SOURCES_PER_ACCOUNT {
            known.record(
                &format!("10.0.0.{i}"),
                "alice",
                t + Duration::seconds(i as i64),
            );
        }
        let later = t + Duration::minutes(1);
        assert!(
            !known.is_known("10.0.0.0", "alice", later),
            "oldest dropped"
        );
        assert!(known.is_known(&format!("10.0.0.{MAX_SOURCES_PER_ACCOUNT}"), "alice", later));

        // Re-recording an existing source moves it to the back instead of duplicating it.
        known.record("10.0.0.1", "alice", later);
        known.record("10.0.0.99", "alice", later);
        assert!(known.is_known("10.0.0.1", "alice", later));
        assert!(!known.is_known("10.0.0.2", "alice", later));
    }

    #[test]
    fn accounts_are_case_sensitive() {
        let known = KnownLoginSources::new(Duration::days(30));
        let t = Utc::now();
        known.record("1.1.1.1", "Alice", t);
        assert!(!known.is_known("1.1.1.1", "alice", t));
    }

    #[test]
    fn expired_sources_are_pruned_on_record() {
        let known = KnownLoginSources::new(Duration::days(30));
        let t = Utc::now();
        known.record("10.0.0.0", "alice", t);
        // One fresh login after the first source expired: with the LRU cap far away, only the
        // expiry pruning can drop the old entry.
        let later = t + Duration::days(31);
        known.record("10.0.0.1", "alice", later);
        let inner = known.inner.lock().unwrap();
        assert_eq!(
            inner.accounts["alice"],
            VecDeque::from([("10.0.0.1".to_string(), later)])
        );
    }

    #[test]
    fn forget_removes_an_account_from_both_indexes() {
        let known = KnownLoginSources::new(Duration::days(30));
        let t = Utc::now();
        known.record("1.1.1.1", "alice", t);
        known.record("1.1.1.1", "bob", t);
        known.forget("alice");
        assert!(!known.is_known("1.1.1.1", "alice", t));
        assert!(known.is_known("1.1.1.1", "bob", t));
        let inner = known.inner.lock().unwrap();
        assert_eq!(inner.order, VecDeque::from(["bob".to_string()]));

        // Forgetting an unknown account is a no-op.
        drop(inner);
        known.forget("nobody");
        assert!(known.is_known("1.1.1.1", "bob", t));
    }
}
