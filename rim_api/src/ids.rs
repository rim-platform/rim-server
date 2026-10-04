use async_graphql::{Error, ID};

pub fn parse_uuid_id<T>(id: &ID) -> Result<T, Error>
where
    T: From<uuid::Uuid>,
{
    let uuid = uuid::Uuid::parse_str(id.as_str()).map_err(|e| Error::new(e.to_string()))?;
    Ok(uuid.into())
}
