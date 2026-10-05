use async_graphql::{Enum, SimpleObject};

/// An optional workspace module (Track 0.4). `INCIDENTS` covers incidents, on-call schedules,
/// escalation policies and alert ingestion together.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum WorkspaceFeature {
    Incidents,
}

impl From<rim_core::workspace_feature::model::WorkspaceFeature> for WorkspaceFeature {
    fn from(value: rim_core::workspace_feature::model::WorkspaceFeature) -> Self {
        match value {
            rim_core::workspace_feature::model::WorkspaceFeature::Incidents => Self::Incidents,
        }
    }
}

impl From<WorkspaceFeature> for rim_core::workspace_feature::model::WorkspaceFeature {
    fn from(value: WorkspaceFeature) -> Self {
        match value {
            WorkspaceFeature::Incidents => Self::Incidents,
        }
    }
}

/// Whether one optional module is on for one workspace. `updatedAt` is null when the value is
/// the feature's default (never explicitly set for this workspace).
#[derive(SimpleObject, Clone, Debug)]
pub struct WorkspaceFeatureState {
    pub feature: WorkspaceFeature,
    pub enabled: bool,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<rim_services::workspace_feature::dto::WorkspaceFeatureStateDto>
    for WorkspaceFeatureState
{
    fn from(value: rim_services::workspace_feature::dto::WorkspaceFeatureStateDto) -> Self {
        Self {
            feature: value.feature.into(),
            enabled: value.enabled,
            updated_at: value.updated_at,
        }
    }
}
