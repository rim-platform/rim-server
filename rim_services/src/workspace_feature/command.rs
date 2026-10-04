use rim_core::server::model::ServerId;
use rim_core::workspace_feature::model::WorkspaceFeature;

use crate::Actor;

/// Any member of `server_id` may list its features.
#[derive(Debug, Clone)]
pub struct ListWorkspaceFeaturesQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

/// Only the server **owner** may turn a feature on or off. Turning a feature off deletes no
/// data; turning it back on restores everything.
#[derive(Debug, Clone)]
pub struct SetWorkspaceFeatureCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub feature: WorkspaceFeature,
    pub enabled: bool,
}
