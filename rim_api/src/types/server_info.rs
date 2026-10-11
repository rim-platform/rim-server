//! GraphQL type describing the running server build and its features.

use async_graphql::*;
use rim_core::server_feature::model::{FeatureState, ServerFeature};
use rim_services::feature::service::FeatureGate;

/// The major version of this GraphQL API. Raised when a field or argument is removed or
/// changes meaning, so a client can tell it is older than the server it talks to.
pub const API_VERSION: i32 = 1;

/// The running server build, the API version it speaks and the optional features it runs.
#[derive(SimpleObject, Clone, Debug)]
pub struct ServerInfo {
    /// The server's release version, e.g. `0.1.0`.
    pub version: String,
    /// The major version of this GraphQL API.
    pub api_version: i32,
    /// Every optional feature with its state on this instance.
    pub features: Vec<ServerFeatureInfo>,
    /// Days a command audit row is kept before it is exported and deleted; null keeps rows
    /// forever.
    pub audit_retention_days: Option<u32>,
}

/// The configured command audit retention in days, held in the schema data; `None` keeps rows
/// forever.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuditRetention(pub Option<u32>);

impl ServerInfo {
    /// This build's version and API version, with the features `gate` reports and the audit
    /// `retention`.
    pub fn current(gate: &dyn FeatureGate, retention: AuditRetention) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            api_version: API_VERSION,
            features: gate
                .states()
                .into_iter()
                .map(|(feature, state)| ServerFeatureInfo::new(feature, &state))
                .collect(),
            audit_retention_days: retention.0,
        }
    }
}

/// One optional feature and whether it works here.
#[derive(SimpleObject, Clone, Debug)]
pub struct ServerFeatureInfo {
    /// The feature's name, as `RIM_FEATURES_DISABLED` takes it: `attachments`, `link_previews`,
    /// `email`, `custom_emoji`, `workspace_plugins` or `ai_commands`. A client ignores a name
    /// it does not know.
    pub key: String,
    pub state: ServerFeatureState,
    /// Why an unavailable feature is unavailable; null otherwise.
    pub reason: Option<String>,
}

impl ServerFeatureInfo {
    fn new(feature: ServerFeature, state: &FeatureState) -> Self {
        Self {
            key: feature.as_str().to_string(),
            state: state.into(),
            reason: state.reason().map(str::to_string),
        }
    }
}

/// Whether an optional feature works on this instance.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ServerFeatureState {
    /// The feature works.
    Enabled,
    /// The operator switched it off.
    Disabled,
    /// The configuration lacks something the feature needs; `reason` says what.
    Unavailable,
}

impl From<&FeatureState> for ServerFeatureState {
    fn from(state: &FeatureState) -> Self {
        match state {
            FeatureState::Enabled => Self::Enabled,
            FeatureState::Disabled => Self::Disabled,
            FeatureState::Unavailable { .. } => Self::Unavailable,
        }
    }
}
