use async_graphql::Error;
use async_graphql::ErrorExtensions;
use async_graphql::Value;
use rim_services::ServiceError;

pub fn map_service_error(err: ServiceError) -> Error {
    match err {
        ServiceError::NotFound => Error::new("not found").extend_with(|_, e| {
            e.set("code", Value::from("NOT_FOUND"));
        }),
        ServiceError::Validation(msg) => Error::new(msg).extend_with(|_, e| {
            e.set("code", Value::from("VALIDATION"));
        }),
        ServiceError::Conflict(msg) => Error::new(msg).extend_with(|_, e| {
            e.set("code", Value::from("CONFLICT"));
        }),
        ServiceError::Unavailable => Error::new("service unavailable").extend_with(|_, e| {
            e.set("code", Value::from("UNAVAILABLE"));
        }),
        ServiceError::Timeout => Error::new("service timeout").extend_with(|_, e| {
            e.set("code", Value::from("TIMEOUT"));
        }),
        ServiceError::PermissionDenied => Error::new("permission denied").extend_with(|_, e| {
            e.set("code", Value::from("FORBIDDEN"));
        }),
        ServiceError::Unauthenticated => Error::new("unauthenticated").extend_with(|_, e| {
            e.set("code", Value::from("UNAUTHENTICATED"));
        }),
        // H1: distinct codes so the frontend can decide whether to retry
        // with a fresh challenge (`CONFIRMATION_STALE`) or surface a
        // generic "confirmation failed" message (`CONFIRMATION_FAILED`).
        ServiceError::ConfirmationStale => Error::new("confirmation expired or was already used")
            .extend_with(|_, e| {
                e.set("code", Value::from("CONFIRMATION_STALE"));
            }),
        ServiceError::ConfirmationFailed(msg) => Error::new(msg).extend_with(|_, e| {
            e.set("code", Value::from("CONFIRMATION_FAILED"));
        }),
        ServiceError::RateLimited {
            retry_after_seconds,
        } => Error::new("rate limit exceeded").extend_with(|_, e| {
            e.set("code", Value::from("RATE_LIMITED"));
            e.set("retryAfterSeconds", Value::from(retry_after_seconds as i64));
        }),
        ServiceError::InvalidToken => Error::new("invalid or unknown token").extend_with(|_, e| {
            e.set("code", Value::from("INVALID_TOKEN"));
        }),
        ServiceError::ExpiredToken => Error::new("token has expired").extend_with(|_, e| {
            e.set("code", Value::from("EXPIRED_TOKEN"));
        }),
        // Distinct from generic CONFLICT so the frontend can highlight the email field (§13.5).
        ServiceError::EmailTaken => Error::new("email already in use").extend_with(|_, e| {
            e.set("code", Value::from("EMAIL_TAKEN"));
        }),
        // Track 0.4: the workspace has this optional module turned off. `feature` carries the
        // snake_case feature name so the client can tell which module to point the owner at.
        ServiceError::FeatureDisabled { feature } => {
            Error::new(format!("workspace feature `{feature}` is disabled")).extend_with(|_, e| {
                e.set("code", Value::from("FEATURE_DISABLED"));
                e.set("feature", Value::from(feature.as_str()));
            })
        }
        // Track 1.1: writes into an archived (read-only) conversation.
        ServiceError::ConversationArchived => {
            Error::new("conversation is archived").extend_with(|_, e| {
                e.set("code", Value::from("CONVERSATION_ARCHIVED"));
            })
        }
        // Track 1.2: the actor is timed out in this workspace. `until` (RFC 3339) lets the
        // client show when writing becomes possible again.
        ServiceError::TimedOut { until } => {
            Error::new(format!("timed out until {}", until.to_rfc3339())).extend_with(|_, e| {
                e.set("code", Value::from("TIMED_OUT"));
                e.set("until", Value::from(until.to_rfc3339()));
            })
        }
        ServiceError::Forbidden(msg) => Error::new(msg).extend_with(|_, e| {
            e.set("code", Value::from("FORBIDDEN"));
        }),
        // The client fetches Commits up to the current epoch and retries.
        ServiceError::StaleEpoch { incarnation, epoch } => Error::new(format!(
            "the group is at incarnation {incarnation}, epoch {epoch}"
        ))
        .extend_with(|_, e| {
            e.set("code", Value::from("STALE_EPOCH"));
            e.set("incarnation", Value::from(incarnation));
            e.set("epoch", Value::from(epoch));
        }),
        ServiceError::MlsCommitRequired { reason } => Error::new("a Commit is required first")
            .extend_with(|_, e| {
                e.set("code", Value::from("MLS_COMMIT_REQUIRED"));
                e.set("reason", Value::from(reason.as_str()));
            }),
        ServiceError::Internal(_) => Error::new("internal error").extend_with(|_, e| {
            e.set("code", Value::from("INTERNAL"));
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_disabled_maps_to_feature_disabled_code_with_feature_name() {
        let err = map_service_error(ServiceError::FeatureDisabled {
            feature: "incidents".to_string(),
        });
        let ext = err.extensions.expect("missing extensions");
        assert_eq!(ext.get("code"), Some(&Value::from("FEATURE_DISABLED")));
        assert_eq!(ext.get("feature"), Some(&Value::from("incidents")));
        assert_eq!(err.message, "workspace feature `incidents` is disabled");
    }

    #[test]
    fn timed_out_maps_to_timed_out_code_with_until() {
        let until = chrono::Utc::now();
        let err = map_service_error(ServiceError::TimedOut { until });
        let ext = err.extensions.expect("missing extensions");
        assert_eq!(ext.get("code"), Some(&Value::from("TIMED_OUT")));
        assert_eq!(ext.get("until"), Some(&Value::from(until.to_rfc3339())));
    }

    #[test]
    fn conversation_archived_maps_to_conversation_archived_code() {
        let err = map_service_error(ServiceError::ConversationArchived);
        let ext = err.extensions.expect("missing extensions");
        assert_eq!(ext.get("code"), Some(&Value::from("CONVERSATION_ARCHIVED")));
    }
}
