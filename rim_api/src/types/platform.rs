use async_graphql::SimpleObject;

#[derive(SimpleObject, Clone, Debug)]
pub struct Capability {
    pub id: String,
    pub title: String,
    pub description: String,
}

impl From<rim_services::capability::dto::CapabilityDto> for Capability {
    fn from(value: rim_services::capability::dto::CapabilityDto) -> Self {
        Self {
            id: value.id,
            title: value.title,
            description: value.description,
        }
    }
}
