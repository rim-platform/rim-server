use async_graphql::{Enum, SimpleObject};

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum IntegrationKind {
    BuiltIn,
    LocalPlugin,
    Bot,
    Webhook,
}

impl From<rim_services::integration::dto::IntegrationKindDto> for IntegrationKind {
    fn from(value: rim_services::integration::dto::IntegrationKindDto) -> Self {
        use rim_services::integration::dto::IntegrationKindDto as D;
        match value {
            D::BuiltIn => Self::BuiltIn,
            D::LocalPlugin => Self::LocalPlugin,
            D::Bot => Self::Bot,
            D::Webhook => Self::Webhook,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum SettingScope {
    User,
    Workspace,
    Integration,
    DeviceLocal,
}

impl From<rim_services::setting::dto::SettingScopeDto> for SettingScope {
    fn from(value: rim_services::setting::dto::SettingScopeDto) -> Self {
        use rim_services::setting::dto::SettingScopeDto as D;
        match value {
            D::User => Self::User,
            D::Workspace => Self::Workspace,
            D::Integration => Self::Integration,
            D::DeviceLocal => Self::DeviceLocal,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Action {
    pub id: String,
    pub title: String,
    pub description: String,
    pub capability_ids: Vec<String>,
}

impl From<rim_services::action::dto::ActionDto> for Action {
    fn from(value: rim_services::action::dto::ActionDto) -> Self {
        Self {
            id: value.id,
            title: value.title,
            description: value.description,
            capability_ids: value.capability_ids,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Event {
    pub id: String,
    pub title: String,
    pub description: String,
}

impl From<rim_services::event::dto::EventDto> for Event {
    fn from(value: rim_services::event::dto::EventDto) -> Self {
        Self {
            id: value.id,
            title: value.title,
            description: value.description,
        }
    }
}

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

#[derive(SimpleObject, Clone, Debug)]
pub struct Integration {
    pub id: String,
    pub kind: IntegrationKind,
    pub title: String,
    pub enabled: bool,
    pub principal_id: Option<async_graphql::ID>,
}

impl From<rim_services::integration::dto::IntegrationDto> for Integration {
    fn from(value: rim_services::integration::dto::IntegrationDto) -> Self {
        Self {
            id: value.id,
            kind: value.kind.into(),
            title: value.title,
            enabled: value.enabled,
            principal_id: value
                .principal_id
                .map(|id| async_graphql::ID(id.to_string())),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Setting {
    pub id: String,
    pub scope: SettingScope,
    pub title: String,
    pub description: String,
    pub value_type: String,
    pub is_secret: bool,
}

impl From<rim_services::setting::dto::SettingDto> for Setting {
    fn from(value: rim_services::setting::dto::SettingDto) -> Self {
        Self {
            id: value.id,
            scope: value.scope.into(),
            title: value.title,
            description: value.description,
            value_type: value.value_type,
            is_secret: value.is_secret,
        }
    }
}
