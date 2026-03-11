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
        ServiceError::Internal(_) => Error::new("internal error").extend_with(|_, e| {
            e.set("code", Value::from("INTERNAL"));
        }),
    }
}
