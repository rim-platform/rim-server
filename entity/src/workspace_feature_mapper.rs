use crate::workspace_feature;
use rim_core::workspace_feature::model::{ParseWorkspaceFeatureError, WorkspaceFeatureState};

/// Fallible: a row may carry a feature string this build does not know (written by a newer
/// server, or a feature since removed). The repository skips such rows.
impl TryFrom<workspace_feature::Model> for WorkspaceFeatureState {
    type Error = ParseWorkspaceFeatureError;

    fn try_from(m: workspace_feature::Model) -> Result<Self, Self::Error> {
        Ok(Self {
            server_id: m.server_id.into(),
            feature: m.feature.parse()?,
            enabled: m.enabled,
            updated_by: m.updated_by.map(Into::into),
            updated_at: m.updated_at.into(),
        })
    }
}
