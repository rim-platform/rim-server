use async_graphql::Error;
use async_graphql::ErrorExtensions;
use async_graphql::Value;
use rim_services::ServiceError;
use rim_services::command::refusal::Refusal;

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
        // Distinct codes so the frontend can decide whether to retry
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
        // Distinct from generic CONFLICT so the frontend can highlight the email field.
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
        // Writes into an archived (read-only) conversation.
        ServiceError::ConversationArchived => {
            Error::new("conversation is archived").extend_with(|_, e| {
                e.set("code", Value::from("CONVERSATION_ARCHIVED"));
            })
        }
        // The actor is timed out in this workspace. `until` (RFC 3339) lets the
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
        // The client says the server is full and points to another server or self-hosting.
        ServiceError::RegistrationFull => Error::new(rim_services::REGISTRATION_FULL_MESSAGE)
            .extend_with(|_, e| {
                e.set("code", Value::from("REGISTRATION_FULL"));
            }),
        // The client offers the reset with the recovery phrase.
        ServiceError::PasswordLocked => Error::new(rim_services::PASSWORD_LOCKED_MESSAGE)
            .extend_with(|_, e| {
                e.set("code", Value::from("PASSWORD_LOCKED"));
            }),
        ServiceError::Internal(_) => Error::new("internal error").extend_with(|_, e| {
            e.set("code", Value::from("INTERNAL"));
        }),
        ServiceError::CapabilityMissing { capability } => Error::new(format!(
            "you do not have the `{capability}` permission"
        ))
        .extend_with(|_, e| {
            e.set("code", Value::from("FORBIDDEN"));
            e.set("missing", Value::from(capability.as_str()));
        }),
        // The code of the failure itself, and whether repeating the call is safe.
        ServiceError::CommandFailed {
            failure,
            retry_safe,
        } => map_service_error(*failure).extend_with(|_, e| {
            e.set("retrySafe", Value::from(retry_safe));
        }),
    }
}

/// [`map_service_error`] for a refused command or macro call, with the `refusal` extension saying
/// what kind of refusal it is, and `retrySafe` on any `FAILED` one.
pub fn map_command_error(err: ServiceError) -> Error {
    let refusal = Refusal::of(&err);
    let retry_safe = rim_services::command::refusal::retry_safe(&err);
    map_service_error(err).extend_with(|_, e| {
        if let Some(refusal) = refusal {
            e.set("refusal", Value::from(refusal.as_str()));
        }
        if let Some(retry_safe) = retry_safe {
            e.set("retrySafe", Value::from(retry_safe));
        }
    })
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
    fn registration_full_maps_to_its_code_with_a_message_pointing_elsewhere() {
        let err = map_service_error(ServiceError::RegistrationFull);
        let ext = err.extensions.expect("missing extensions");
        assert_eq!(ext.get("code"), Some(&Value::from("REGISTRATION_FULL")));
        assert!(err.message.contains("this server is full"));
        assert!(err.message.contains("run your own server"));
    }

    #[test]
    fn password_locked_maps_to_its_code_with_a_message_naming_the_recovery_phrase() {
        let err = map_service_error(ServiceError::PasswordLocked);
        let ext = err.extensions.expect("missing extensions");
        assert_eq!(ext.get("code"), Some(&Value::from("PASSWORD_LOCKED")));
        assert!(err.message.contains("recovery phrase"));
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

    fn ext(err: Error) -> async_graphql::ErrorExtensionValues {
        err.extensions.expect("missing extensions")
    }

    #[test]
    fn a_missing_capability_is_denied_and_names_the_capability() {
        let e = ext(map_command_error(ServiceError::CapabilityMissing {
            capability: "delete_member_messages".to_string(),
        }));
        assert_eq!(e.get("code"), Some(&Value::from("FORBIDDEN")));
        assert_eq!(e.get("refusal"), Some(&Value::from("DENIED")));
        assert_eq!(
            e.get("missing"),
            Some(&Value::from("delete_member_messages"))
        );
        assert_eq!(e.get("retrySafe"), None);
    }

    #[test]
    fn a_bare_permission_denied_is_denied_and_names_nothing() {
        let e = ext(map_command_error(ServiceError::PermissionDenied));
        assert_eq!(e.get("refusal"), Some(&Value::from("DENIED")));
        assert_eq!(e.get("missing"), None);
    }

    #[test]
    fn a_failure_keeps_its_code_and_says_whether_a_retry_is_safe() {
        let e = ext(map_command_error(ServiceError::CommandFailed {
            failure: Box::new(ServiceError::Timeout),
            retry_safe: true,
        }));
        assert_eq!(e.get("code"), Some(&Value::from("TIMEOUT")));
        assert_eq!(e.get("refusal"), Some(&Value::from("FAILED")));
        assert_eq!(e.get("retrySafe"), Some(&Value::from(true)));

        let e = ext(map_command_error(ServiceError::CommandFailed {
            failure: Box::new(ServiceError::Unavailable),
            retry_safe: false,
        }));
        assert_eq!(e.get("code"), Some(&Value::from("UNAVAILABLE")));
        assert_eq!(e.get("retrySafe"), Some(&Value::from(false)));
    }

    #[test]
    fn a_rate_limited_call_is_a_failure_that_is_safe_to_retry() {
        let e = ext(map_command_error(ServiceError::RateLimited {
            retry_after_seconds: 7,
        }));
        assert_eq!(e.get("refusal"), Some(&Value::from("FAILED")));
        assert_eq!(e.get("retrySafe"), Some(&Value::from(true)));
        assert_eq!(e.get("retryAfterSeconds"), Some(&Value::from(7)));
    }

    #[test]
    fn a_failure_with_no_answer_of_its_own_is_not_safe_to_retry() {
        for err in [
            ServiceError::Unavailable,
            ServiceError::Timeout,
            ServiceError::Internal(None),
            ServiceError::upstream("rims1_h"),
        ] {
            let e = ext(map_command_error(err));
            assert_eq!(e.get("refusal"), Some(&Value::from("FAILED")));
            assert_eq!(e.get("retrySafe"), Some(&Value::from(false)));
        }
    }

    #[test]
    fn a_stale_confirmation_is_expired() {
        let e = ext(map_command_error(ServiceError::ConfirmationStale));
        assert_eq!(e.get("code"), Some(&Value::from("CONFIRMATION_STALE")));
        assert_eq!(e.get("refusal"), Some(&Value::from("EXPIRED")));
    }

    #[test]
    fn a_timeout_is_denied_and_keeps_until() {
        let until = chrono::Utc::now();
        let e = ext(map_command_error(ServiceError::TimedOut { until }));
        assert_eq!(e.get("refusal"), Some(&Value::from("DENIED")));
        assert_eq!(e.get("until"), Some(&Value::from(until.to_rfc3339())));
    }

    #[test]
    fn a_wrong_call_is_invalid() {
        for err in [
            ServiceError::Validation("bad".to_string()),
            ServiceError::NotFound,
            ServiceError::Conflict("clash".to_string()),
        ] {
            let e = ext(map_command_error(err));
            assert_eq!(e.get("refusal"), Some(&Value::from("INVALID")));
        }
    }

    #[test]
    fn other_errors_have_no_refusal() {
        let e = ext(map_command_error(ServiceError::Unauthenticated));
        assert_eq!(e.get("refusal"), None);
    }

    #[test]
    fn map_service_error_alone_adds_no_refusal() {
        let e = ext(map_service_error(ServiceError::PermissionDenied));
        assert_eq!(e.get("refusal"), None);
    }
}
