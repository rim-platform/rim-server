#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SettingScopeDto {
    User,
    Workspace,
    Integration,
    DeviceLocal,
}

impl From<rim_core::setting::model::SettingScope> for SettingScopeDto {
    fn from(value: rim_core::setting::model::SettingScope) -> Self {
        match value {
            rim_core::setting::model::SettingScope::User => Self::User,
            rim_core::setting::model::SettingScope::Workspace => Self::Workspace,
            rim_core::setting::model::SettingScope::Integration => Self::Integration,
            rim_core::setting::model::SettingScope::DeviceLocal => Self::DeviceLocal,
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SettingDto {
    pub id: String,
    pub scope: SettingScopeDto,
    pub title: String,
    pub description: String,
    pub value_type: String,
    pub is_secret: bool,
}

impl From<rim_core::setting::model::Setting> for SettingDto {
    fn from(value: rim_core::setting::model::Setting) -> Self {
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
