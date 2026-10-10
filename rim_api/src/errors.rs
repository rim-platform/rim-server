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
        // A peer server this request needed could not be reached.
        // `hostServerId` is null when only the origin it was sought at is known.
        ServiceError::UpstreamUnavailable {
            host_server_id,
            origin,
        } => Error::new(rim_services::upstream_message(
            host_server_id.as_deref(),
            origin.as_deref(),
        ))
        .extend_with(|_, e| {
            e.set("code", Value::from("UPSTREAM_UNAVAILABLE"));
            e.set(
                "hostServerId",
                host_server_id.as_deref().map_or(Value::Null, Value::from),
            );
            if let Some(origin) = origin.as_deref() {
                e.set("origin", Value::from(origin));
            }
        }),
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
        // The client reads `serverInfo.features` for the state and its reason.
        ServiceError::FeatureDisabled(feature) => Error::new(format!(
            "the {feature} feature is not available on this server"
        ))
        .extend_with(|_, e| {
            e.set("code", Value::from("FEATURE_DISABLED"));
            e.set("feature", Value::from(feature.as_str()));
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
    fn feature_disabled_maps_to_feature_disabled_code_with_the_feature() {
        let err = map_service_error(ServiceError::FeatureDisabled(
            rim_core::server_feature::model::ServerFeature::CustomEmoji,
        ));
        let ext = err.extensions.expect("missing extensions");
        assert_eq!(ext.get("code"), Some(&Value::from("FEATURE_DISABLED")));
        assert_eq!(ext.get("feature"), Some(&Value::from("custom_emoji")));
        assert_eq!(
            err.message,
            "the custom_emoji feature is not available on this server"
        );
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
    fn upstream_unavailable_names_the_server_id_or_else_the_origin() {
        let err = map_service_error(ServiceError::upstream("rims1_h"));
        let ext = err.extensions.expect("missing extensions");
        assert_eq!(ext.get("code"), Some(&Value::from("UPSTREAM_UNAVAILABLE")));
        assert_eq!(ext.get("hostServerId"), Some(&Value::from("rims1_h")));
        assert_eq!(ext.get("origin"), None);
        assert_eq!(err.message, "server rims1_h is unavailable");

        let err = map_service_error(ServiceError::upstream_origin("https://relay.example"));
        let ext = err.extensions.expect("missing extensions");
        assert_eq!(ext.get("code"), Some(&Value::from("UPSTREAM_UNAVAILABLE")));
        assert_eq!(ext.get("hostServerId"), Some(&Value::Null));
        assert_eq!(
            ext.get("origin"),
            Some(&Value::from("https://relay.example"))
        );
        assert_eq!(
            err.message,
            "the server at https://relay.example is unavailable"
        );
    }

    #[test]
    fn conversation_archived_maps_to_conversation_archived_code() {
        let err = map_service_error(ServiceError::ConversationArchived);
        let ext = err.extensions.expect("missing extensions");
        assert_eq!(ext.get("code"), Some(&Value::from("CONVERSATION_ARCHIVED")));
    }
}
