use rim_core::server::model::ServerId;
use rim_core::workspace_plugin::model::WorkspacePluginStatus;

use crate::Actor;
use crate::key_auth::command::SignedPayload;

#[derive(Debug, Clone)]
pub struct ListWorkspacePluginsQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

#[derive(Debug, Clone)]
pub struct AddWorkspacePluginCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    /// The publisher's `plugin-manifest` statement.
    pub statement: SignedPayload,
    pub status: WorkspacePluginStatus,
    /// Where clients fetch a bundle source; `None` for this server's copy.
    pub bundle_url: Option<String>,
    /// The publisher's key log, for a publisher not enrolled on this server.
    pub publisher_key_log: Option<Vec<SignedPayload>>,
}

/// Changes an entry's statement, status or bundle URL; at least one of them.
#[derive(Debug, Clone)]
pub struct UpdateWorkspacePluginCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub plugin_id: String,
    pub statement: Option<SignedPayload>,
    pub status: Option<WorkspacePluginStatus>,
    pub bundle_url: Option<String>,
    pub publisher_key_log: Option<Vec<SignedPayload>>,
}

#[derive(Debug, Clone)]
pub struct RemoveWorkspacePluginCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub plugin_id: String,
}

#[derive(Debug, Clone)]
pub struct UploadPluginBundleCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    /// The bundle file exactly as received.
    pub bytes: Vec<u8>,
    /// SHA-256 of `bytes` as lowercase hex, computed by the transport while it read them.
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct DownloadPluginBundleQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub sha256: String,
}

/// Fetch a pinned web bundle without storing it in any workspace.
#[derive(Debug, Clone)]
pub struct FetchPluginBundleCmd {
    pub actor: Actor,
    pub url: String,
    pub sha256: String,
}
