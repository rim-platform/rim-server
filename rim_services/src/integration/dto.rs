#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IntegrationKindDto {
    BuiltIn,
    LocalPlugin,
    Bot,
    Webhook,
}

impl From<rim_core::integration::model::IntegrationKind> for IntegrationKindDto {
    fn from(value: rim_core::integration::model::IntegrationKind) -> Self {
        match value {
            rim_core::integration::model::IntegrationKind::BuiltIn => Self::BuiltIn,
            rim_core::integration::model::IntegrationKind::LocalPlugin => Self::LocalPlugin,
            rim_core::integration::model::IntegrationKind::Bot => Self::Bot,
            rim_core::integration::model::IntegrationKind::Webhook => Self::Webhook,
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IntegrationDto {
    pub id: String,
    pub kind: IntegrationKindDto,
    pub title: String,
    pub enabled: bool,
    pub principal_id: Option<rim_core::principal::model::PrincipalId>,
}

impl From<rim_core::integration::model::Integration> for IntegrationDto {
    fn from(value: rim_core::integration::model::Integration) -> Self {
        Self {
            id: value.id,
            kind: value.kind.into(),
            title: value.title,
            enabled: value.enabled,
            principal_id: value.principal_id,
        }
    }
}
