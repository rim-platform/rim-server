use crate::principal::model::PrincipalId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntegrationKind {
    BuiltIn,
    LocalPlugin,
    Bot,
    Webhook,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Integration {
    pub id: String,
    pub kind: IntegrationKind,
    pub title: String,
    pub enabled: bool,
    pub principal_id: Option<PrincipalId>,
}
