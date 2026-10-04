use chrono::{DateTime, Utc};

use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_core::workspace_feature::model::{WorkspaceFeature, WorkspaceFeatureState};

/// The resolved state of one feature for one workspace. When no row has been written for the
/// feature, `enabled` is [`WorkspaceFeature::default_enabled`] and `updated_by`/`updated_at`
/// are `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WorkspaceFeatureStateDto {
    pub server_id: ServerId,
    pub feature: WorkspaceFeature,
    pub enabled: bool,
    pub updated_by: Option<UserId>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl WorkspaceFeatureStateDto {
    /// The state a workspace has for `feature` when nothing has been persisted.
    pub fn default_for(server_id: ServerId, feature: WorkspaceFeature) -> Self {
        Self {
            server_id,
            feature,
            enabled: feature.default_enabled(),
            updated_by: None,
            updated_at: None,
        }
    }
}

impl From<WorkspaceFeatureState> for WorkspaceFeatureStateDto {
    fn from(s: WorkspaceFeatureState) -> Self {
        Self {
            server_id: s.server_id,
            feature: s.feature,
            enabled: s.enabled,
            updated_by: s.updated_by,
            updated_at: Some(s.updated_at),
        }
    }
}
