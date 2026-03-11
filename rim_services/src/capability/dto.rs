#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CapabilityDto {
    pub id: String,
    pub title: String,
    pub description: String,
}

impl From<rim_core::capability::model::Capability> for CapabilityDto {
    fn from(value: rim_core::capability::model::Capability) -> Self {
        Self {
            id: value.id,
            title: value.title,
            description: value.description,
        }
    }
}
