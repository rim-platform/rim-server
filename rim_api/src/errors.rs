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
        ServiceError::Internal(_) => Error::new("internal error").extend_with(|_, e| {
            e.set("code", Value::from("INTERNAL"));
        }),
    }
}
