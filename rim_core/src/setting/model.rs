#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingScope {
    User,
    Workspace,
    Integration,
    DeviceLocal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub id: String,
    pub scope: SettingScope,
    pub title: String,
    pub description: String,
    pub value_type: String,
    pub is_secret: bool,
}
