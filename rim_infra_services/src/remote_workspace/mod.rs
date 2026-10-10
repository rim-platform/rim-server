//! Workspace members whose relay is another server. The host side admits them through their
//! relay, keeps the delegations that let a relay carry their traffic, applies presence their
//! relay reports and revocations relays and apps bring. The relay side joins its users to
//! workspaces elsewhere, renews their delegations, reports their presence and sends their
//! device revocations to every host holding them.

mod host;
#[cfg(test)]
mod host_tests;
pub(crate) mod presence;
mod relay;

use chrono::{DateTime, Duration, Utc};
use rim_services::ServiceError;
use rim_services::s2s::dto::S2sResponse;

pub use host::{DefaultWorkspaceHost, RepoDelegationCheck, WorkspaceHostRepos};
pub use relay::{DefaultWorkspaceRelay, WorkspaceRelayRepos};

/// Longest a host honours a delegation after accepting it, whatever it says.
pub(crate) const MAX_ACCEPTED_LIFETIME: Duration = Duration::hours(24);
/// Most delegations one subscriptions request carries.
pub(crate) const MAX_DELEGATIONS_PER_REQUEST: usize = 64;
/// How long a revocation keeps being sent to a host that has not acknowledged it.
pub(crate) const REVOCATION_FANOUT_WINDOW: Duration = Duration::hours(24);
/// Seconds after the revocation at which the first retries go; after the last, every 15
/// minutes.
const REVOCATION_RETRY_OFFSETS: [i64; 5] = [10, 30, 60, 120, 300];
const REVOCATION_RETRY_EVERY: i64 = 15 * 60;

/// When to send a revocation again after `attempts` attempts, counted from `revoked_at`: at
/// 10 s, 30 s, 1 min, 2 min and 5 min, then every 15 minutes. `None` once that passes 24 hours.
pub(crate) fn next_revocation_attempt(
    revoked_at: DateTime<Utc>,
    attempts: i32,
) -> Option<DateTime<Utc>> {
    let attempts = usize::try_from(attempts).ok()?;
    let offset = match attempts {
        0 => 0,
        n if n <= REVOCATION_RETRY_OFFSETS.len() => REVOCATION_RETRY_OFFSETS[n - 1],
        n => {
            let past = i64::try_from(n - REVOCATION_RETRY_OFFSETS.len()).ok()?;
            REVOCATION_RETRY_OFFSETS[REVOCATION_RETRY_OFFSETS.len() - 1]
                + past.checked_mul(REVOCATION_RETRY_EVERY)?
        }
    };
    let offset = Duration::seconds(offset);
    (offset < REVOCATION_FANOUT_WINDOW).then(|| revoked_at + offset)
}

/// A peer's refusal as the service error it means; a status with no meaning here is the peer
/// being unavailable.
pub(crate) fn peer_refusal(peer_server_id: &str, response: &S2sResponse) -> ServiceError {
    let message = serde_json::from_slice::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|v| v.get("message")?.as_str().map(str::to_string))
        .unwrap_or_else(|| "refused by the host".to_string());
    match response.status {
        400 => ServiceError::Validation(message),
        401 | 403 => ServiceError::Forbidden(message),
        404 => ServiceError::NotFound,
        409 => ServiceError::Conflict(message),
        429 => ServiceError::RateLimited {
            retry_after_seconds: 60,
        },
        _ => ServiceError::UpstreamUnavailable {
            host_server_id: peer_server_id.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_791_504_000 + seconds, 0).expect("time")
    }

    #[test]
    fn revocations_are_retried_on_the_schedule_until_a_day_passes() {
        let revoked = at(0);
        let offsets: Vec<i64> = (0..8)
            .map(|n| {
                (next_revocation_attempt(revoked, n).expect("within a day") - revoked).num_seconds()
            })
            .collect();
        assert_eq!(offsets, vec![0, 10, 30, 60, 120, 300, 1200, 2100]);
        // 300 s + 95 × 15 min is 23 h 50 min; the next would be past 24 hours.
        assert_eq!(
            next_revocation_attempt(revoked, 100),
            Some(revoked + Duration::seconds(300 + 95 * 900))
        );
        assert_eq!(next_revocation_attempt(revoked, 101), None);
        assert_eq!(next_revocation_attempt(revoked, -1), None);
    }

    #[test]
    fn peer_refusals_keep_their_meaning() {
        let response = |status: u16| S2sResponse {
            status,
            content_type: "application/json".to_string(),
            body: br#"{"error":"forbidden","message":"not a member"}"#.to_vec(),
            key_seq: 0,
            headers: Vec::new(),
        };
        assert!(matches!(
            peer_refusal("rims1_h", &response(403)),
            ServiceError::Forbidden(m) if m == "not a member"
        ));
        assert!(matches!(
            peer_refusal("rims1_h", &response(404)),
            ServiceError::NotFound
        ));
        assert!(matches!(
            peer_refusal("rims1_h", &response(500)),
            ServiceError::UpstreamUnavailable { .. }
        ));
    }
}
