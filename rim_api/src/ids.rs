use async_graphql::{Error, ID};
use rim_services::ServiceError;

use crate::errors::map_service_error;

/// Parse a UUID-backed id; a malformed one is a `VALIDATION` error.
pub fn parse_uuid_id<T>(id: &ID) -> Result<T, Error>
where
    T: From<uuid::Uuid>,
{
    let uuid = uuid::Uuid::parse_str(id.as_str())
        .map_err(|e| map_service_error(ServiceError::Validation(e.to_string())))?;
    Ok(uuid.into())
}
