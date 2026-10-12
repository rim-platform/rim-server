//! A token bucket rate limiter keyed by any hashable value, holding a bounded number of keys.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use rim_services::command::rate_limit::RateLimitDecision;

/// Keys a limiter tracks before it evicts the oldest.
pub const DEFAULT_MAX_KEYS: usize = 100_000;

/// How often buckets that have refilled completely are dropped.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// A bucket's refill rate and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BucketRate {
    /// Tokens added per minute.
    pub per_minute: u32,
    /// Most tokens a bucket holds: the largest burst.
    pub burst: u32,
}

impl BucketRate {
    /// `per_minute` tokens a minute with a burst of `burst`; `None` when either is 0.
    pub fn new(per_minute: u32, burst: u32) -> Option<Self> {
        (per_minute > 0 && burst > 0).then_some(Self { per_minute, burst })
    }

    /// `per_minute` tokens a minute with a burst of the same size; `None` for 0.
    pub fn per_minute(per_minute: u32) -> Option<Self> {
        Self::new(per_minute, per_minute)
    }

    fn per_second(self) -> f64 {
        f64::from(self.per_minute) / 60.0
    }

    fn capacity(self) -> f64 {
        f64::from(self.burst)
    }
}

struct Bucket {
    tokens: f64,
    updated: Instant,
}

impl Bucket {
    /// Add the tokens earned since the last update, up to the capacity.
    fn refill(&mut self, rate: BucketRate, now: Instant) {
        let elapsed = now.saturating_duration_since(self.updated);
        self.tokens =
            (self.tokens + elapsed.as_secs_f64() * rate.per_second()).min(rate.capacity());
        if now > self.updated {
            self.updated = now;
        }
    }
}

/// The buckets and the order their keys were added in, for eviction.
struct Buckets<K> {
    map: HashMap<K, Bucket>,
    order: VecDeque<K>,
    swept: Option<Instant>,
}

/// Token buckets per key, refilled continuously at one rate. A key without a bucket has a full
/// one; a bucket that refills completely is dropped at the next sweep, and past `max_keys` the
/// oldest key is evicted, so memory stays bounded whatever keys callers send. The caller passes
/// the time, so tests drive the clock. In memory, per node.
pub struct TokenBucketLimiter<K> {
    rate: BucketRate,
    max_keys: usize,
    buckets: Mutex<Buckets<K>>,
}

impl<K: Hash + Eq + Clone> TokenBucketLimiter<K> {
    pub fn new(rate: BucketRate) -> Self {
        Self::with_max_keys(rate, DEFAULT_MAX_KEYS)
    }

    pub fn with_max_keys(rate: BucketRate, max_keys: usize) -> Self {
        Self {
            rate,
            max_keys: max_keys.max(1),
            buckets: Mutex::new(Buckets {
                map: HashMap::new(),
                order: VecDeque::new(),
                swept: None,
            }),
        }
    }

    pub fn rate(&self) -> BucketRate {
        self.rate
    }

    /// Take one token from `key`'s bucket.
    pub fn check(&self, key: &K, now: Instant) -> RateLimitDecision {
        self.take(key, 1, now)
    }

    /// Take `cost` tokens from `key`'s bucket, or none and the seconds until enough are back.
    /// A cost above the burst never passes.
    pub fn take(&self, key: &K, cost: u32, now: Instant) -> RateLimitDecision {
        let rate = self.rate;
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        buckets.sweep_if_due(rate, now);
        let tokens = match buckets.map.get_mut(key) {
            Some(bucket) => {
                bucket.refill(rate, now);
                bucket.tokens
            }
            None => rate.capacity(),
        };
        let cost = f64::from(cost);
        if cost > tokens {
            let missing = cost.min(rate.capacity()) - tokens;
            let seconds = (missing / rate.per_second()).ceil();
            return RateLimitDecision::Limited {
                retry_after_seconds: (seconds as u64).max(1),
            };
        }
        match buckets.map.get_mut(key) {
            Some(bucket) => bucket.tokens -= cost,
            None => {
                buckets.map.insert(
                    key.clone(),
                    Bucket {
                        tokens: rate.capacity() - cost,
                        updated: now,
                    },
                );
                buckets.order.push_back(key.clone());
                while buckets.map.len() > self.max_keys {
                    let Some(oldest) = buckets.order.pop_front() else {
                        break;
                    };
                    buckets.map.remove(&oldest);
                }
            }
        }
        RateLimitDecision::Allowed
    }
}

impl<K: Hash + Eq + Clone> Buckets<K> {
    /// Drop the buckets that are full again, once per [`SWEEP_INTERVAL`].
    fn sweep_if_due(&mut self, rate: BucketRate, now: Instant) {
        match self.swept {
            None => {
                self.swept = Some(now);
                return;
            }
            Some(at) if now.saturating_duration_since(at) < SWEEP_INTERVAL => return,
            Some(_) => {}
        }
        self.swept = Some(now);
        self.map.retain(|_, bucket| {
            bucket.refill(rate, now);
            bucket.tokens < rate.capacity()
        });
        let map = &self.map;
        self.order.retain(|key| map.contains_key(key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(per_minute: u32, burst: u32) -> BucketRate {
        BucketRate::new(per_minute, burst).expect("a rate")
    }

    fn limited(decision: RateLimitDecision) -> u64 {
        match decision {
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => retry_after_seconds,
            RateLimitDecision::Allowed => panic!("expected a refusal"),
        }
    }

    #[test]
    fn a_zero_rate_or_burst_is_no_limit() {
        assert_eq!(BucketRate::new(0, 5), None);
        assert_eq!(BucketRate::new(5, 0), None);
        assert_eq!(BucketRate::per_minute(0), None);
        assert_eq!(
            BucketRate::per_minute(3),
            Some(BucketRate {
                per_minute: 3,
                burst: 3
            })
        );
    }

    #[test]
    fn the_burst_passes_then_tokens_come_back_at_the_rate() {
        let limiter = TokenBucketLimiter::new(rate(60, 3));
        let t0 = Instant::now();
        for _ in 0..3 {
            assert_eq!(limiter.check(&"a", t0), RateLimitDecision::Allowed);
        }
        assert_eq!(limited(limiter.check(&"a", t0)), 1);
        assert_eq!(limiter.check(&"b", t0), RateLimitDecision::Allowed);
        // One token a second: two seconds later two more pass.
        let t2 = t0 + Duration::from_secs(2);
        assert_eq!(limiter.check(&"a", t2), RateLimitDecision::Allowed);
        assert_eq!(limiter.check(&"a", t2), RateLimitDecision::Allowed);
        assert!(matches!(
            limiter.check(&"a", t2),
            RateLimitDecision::Limited { .. }
        ));
        // A long pause refills to the burst, not beyond it.
        let later = t0 + Duration::from_secs(3600);
        for _ in 0..3 {
            assert_eq!(limiter.check(&"a", later), RateLimitDecision::Allowed);
        }
        assert!(matches!(
            limiter.check(&"a", later),
            RateLimitDecision::Limited { .. }
        ));
    }

    #[test]
    fn the_retry_after_is_the_time_until_enough_tokens_are_back() {
        let limiter = TokenBucketLimiter::new(rate(3, 3));
        let t0 = Instant::now();
        assert_eq!(limiter.take(&"a", 3, t0), RateLimitDecision::Allowed);
        // One token every 20 seconds.
        assert_eq!(limited(limiter.check(&"a", t0)), 20);
        assert_eq!(limited(limiter.take(&"a", 2, t0)), 40);
        assert_eq!(
            limiter.check(&"a", t0 + Duration::from_secs(20)),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn a_cost_above_the_burst_never_passes() {
        let limiter = TokenBucketLimiter::new(rate(60, 5));
        let t0 = Instant::now();
        assert!(matches!(
            limiter.take(&"a", 6, t0),
            RateLimitDecision::Limited { .. }
        ));
        assert!(matches!(
            limiter.take(&"a", 6, t0 + Duration::from_secs(3600)),
            RateLimitDecision::Limited { .. }
        ));
        assert_eq!(limiter.take(&"a", 5, t0), RateLimitDecision::Allowed);
    }

    #[test]
    fn past_the_key_cap_the_oldest_key_is_evicted() {
        let limiter = TokenBucketLimiter::with_max_keys(rate(1, 1), 2);
        let t0 = Instant::now();
        for key in ["a", "b", "c"] {
            assert_eq!(limiter.check(&key, t0), RateLimitDecision::Allowed);
        }
        {
            let buckets = limiter.buckets.lock().expect("lock");
            assert_eq!(buckets.map.len(), 2);
            assert!(!buckets.map.contains_key("a"));
            assert_eq!(buckets.order, ["b", "c"]);
        }
        // "a" was forgotten, so it has a full bucket again; "c" is still empty.
        assert_eq!(limiter.check(&"a", t0), RateLimitDecision::Allowed);
        assert!(matches!(
            limiter.check(&"c", t0),
            RateLimitDecision::Limited { .. }
        ));
    }

    #[test]
    fn buckets_that_refilled_are_dropped_by_the_sweep() {
        let limiter = TokenBucketLimiter::new(rate(60, 60));
        let t0 = Instant::now();
        assert_eq!(limiter.check(&"idle", t0), RateLimitDecision::Allowed);
        assert_eq!(limiter.take(&"busy", 60, t0), RateLimitDecision::Allowed);
        // A minute later "idle" is full again and "busy" has refilled too; "busy" spends again.
        let t1 = t0 + SWEEP_INTERVAL;
        assert_eq!(limiter.take(&"busy", 60, t1), RateLimitDecision::Allowed);
        let buckets = limiter.buckets.lock().expect("lock");
        assert_eq!(buckets.map.len(), 1);
        assert!(buckets.map.contains_key("busy"));
        assert_eq!(buckets.order, ["busy"]);
    }
}
