//! What kind of refusal a failed command call is, for every transport that runs commands.

use crate::ServiceError;

/// What the caller of a refused command call can do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A policy decision: repeating the call does not help.
    Denied,
    /// A technical problem, not a decision.
    Failed,
    /// The confirmation expired or was already used. Nothing ran.
    Expired,
    /// The call itself is wrong: bad arguments, nothing by that name, or a conflict.
    Invalid,
}

impl Refusal {
    /// The name a transport shows: `DENIED`, `FAILED`, `EXPIRED` or `INVALID`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Denied => "DENIED",
            Self::Failed => "FAILED",
            Self::Expired => "EXPIRED",
            Self::Invalid => "INVALID",
        }
    }

    /// The kind of refusal `err` is, or `None` for an error a command call does not end on.
    pub fn of(err: &ServiceError) -> Option<Self> {
        match err {
            ServiceError::PermissionDenied
            | ServiceError::CapabilityMissing { .. }
            | ServiceError::Forbidden(_)
            | ServiceError::TimedOut { .. }
            | ServiceError::FeatureDisabled(_)
            | ServiceError::ConversationArchived
            | ServiceError::ConfirmationFailed(_) => Some(Self::Denied),
            ServiceError::CommandFailed { .. }
            | ServiceError::RateLimited { .. }
            | ServiceError::Unavailable
            | ServiceError::Timeout
            | ServiceError::Internal(_)
            | ServiceError::UpstreamUnavailable { .. } => Some(Self::Failed),
            ServiceError::ConfirmationStale => Some(Self::Expired),
            ServiceError::Validation(_) | ServiceError::NotFound | ServiceError::Conflict(_) => {
                Some(Self::Invalid)
            }
            _ => None,
        }
    }
}

/// Whether repeating the call that ended on `err` cannot change anything twice, for a technical
/// failure; `None` for a refusal that is a decision. A rate-limited call never started. A failure
/// with no answer of its own, as a macro's can be, may have changed something.
pub fn retry_safe(err: &ServiceError) -> Option<bool> {
    match err {
        ServiceError::CommandFailed { retry_safe, .. } => Some(*retry_safe),
        ServiceError::RateLimited { .. } => Some(true),
        ServiceError::Unavailable
        | ServiceError::Timeout
        | ServiceError::Internal(_)
        | ServiceError::UpstreamUnavailable { .. } => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_error_of_a_command_call_has_its_kind() {
        let until = chrono::Utc::now();
        for (err, kind) in [
            (ServiceError::PermissionDenied, Refusal::Denied),
            (ServiceError::TimedOut { until }, Refusal::Denied),
            (ServiceError::ConversationArchived, Refusal::Denied),
            (ServiceError::Timeout, Refusal::Failed),
            (
                ServiceError::RateLimited {
                    retry_after_seconds: 1,
                },
                Refusal::Failed,
            ),
            (ServiceError::ConfirmationStale, Refusal::Expired),
            (ServiceError::NotFound, Refusal::Invalid),
            (
                ServiceError::Validation("bad".to_string()),
                Refusal::Invalid,
            ),
        ] {
            assert_eq!(Refusal::of(&err), Some(kind), "{err:?}");
        }
        assert_eq!(Refusal::of(&ServiceError::Unauthenticated), None);
    }

    #[test]
    fn only_a_stopped_or_a_reading_call_is_safe_to_retry() {
        assert_eq!(
            retry_safe(&ServiceError::RateLimited {
                retry_after_seconds: 1
            }),
            Some(true)
        );
        assert_eq!(
            retry_safe(&ServiceError::CommandFailed {
                failure: Box::new(ServiceError::Timeout),
                retry_safe: true,
            }),
            Some(true)
        );
        assert_eq!(retry_safe(&ServiceError::Timeout), Some(false));
        assert_eq!(retry_safe(&ServiceError::PermissionDenied), None);
    }
}
