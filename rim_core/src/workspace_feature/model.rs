use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};

use crate::server::model::ServerId;
use crate::user::model::UserId;

/// An optional module a workspace (server) can turn on or off. Persisted as the snake_case
/// string returned by [`Self::as_str`] in `workspace_feature.feature`.
///
/// A workspace with no row for a feature gets [`Self::default_enabled`]; the
/// `add_workspace_feature` migration's data step is what keeps pre-existing workspaces on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum WorkspaceFeature {
    /// Incidents, on-call schedules, escalation policies and alert ingestion, as one unit.
    Incidents,
}

impl WorkspaceFeature {
    /// Every known feature, in display order.
    pub const ALL: [WorkspaceFeature; 1] = [WorkspaceFeature::Incidents];

    pub fn as_str(&self) -> &'static str {
        match self {
            WorkspaceFeature::Incidents => "incidents",
        }
    }

    /// The value a workspace gets when no row exists for this feature.
    pub fn default_enabled(&self) -> bool {
        match self {
            WorkspaceFeature::Incidents => Self::INCIDENTS_DEFAULT_ENABLED,
        }
    }

    /// Default for [`WorkspaceFeature::Incidents`] on workspaces created after the
    /// `add_workspace_feature` migration: off (roadmap Track 0.4, decision 4).
    pub const INCIDENTS_DEFAULT_ENABLED: bool = false;
}

impl fmt::Display for WorkspaceFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for WorkspaceFeature {
    type Err = ParseWorkspaceFeatureError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "incidents" => Ok(WorkspaceFeature::Incidents),
            other => Err(ParseWorkspaceFeatureError::new(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseWorkspaceFeatureError {
    input: String,
}

impl ParseWorkspaceFeatureError {
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }
}

impl fmt::Display for ParseWorkspaceFeatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown workspace feature: {}", self.input)
    }
}

impl std::error::Error for ParseWorkspaceFeatureError {}

/// The persisted on/off state of one feature for one workspace. `updated_by` is `None` for
/// rows written by the migration's data step (and after the updating user is deleted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFeatureState {
    pub server_id: ServerId,
    pub feature: WorkspaceFeature,
    pub enabled: bool,
    pub updated_by: Option<UserId>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_round_trips_through_from_str() {
        for feature in WorkspaceFeature::ALL {
            assert_eq!(feature.as_str().parse::<WorkspaceFeature>(), Ok(feature));
            assert_eq!(feature.to_string(), feature.as_str());
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        let err = "billing".parse::<WorkspaceFeature>().unwrap_err();
        assert_eq!(err.to_string(), "unknown workspace feature: billing");
    }

    #[test]
    fn incidents_default_is_off() {
        assert!(!WorkspaceFeature::Incidents.default_enabled());
    }
}
