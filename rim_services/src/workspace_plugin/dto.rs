use chrono::{DateTime, Utc};

use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_core::workspace_plugin::model::{
    PluginRuntimeKind, PluginSource, WorkspacePluginId, WorkspacePluginStatus,
};

use crate::key_auth::command::SignedPayload;

/// Whether an entry's statement still stands, judged when the entry is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginSignatureStatus {
    Valid,
    /// A recovery rotation follows the statement's `keySeq` in the publisher's key log.
    PublisherRecovered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePluginDto {
    pub id: WorkspacePluginId,
    pub server_id: ServerId,
    pub plugin_id: String,
    pub version: String,
    pub display_name: String,
    pub runtime_kind: PluginRuntimeKind,
    pub status: WorkspacePluginStatus,
    pub source: PluginSource,
    /// The exact `rim-plugin.json` text inside the statement.
    pub manifest: String,
    pub statement: SignedPayload,
    pub publisher_principal_id: String,
    pub publisher_name: String,
    pub publisher_key_seq: u32,
    /// This server's key log of the publisher when they are enrolled here, else the stored one.
    pub publisher_key_log: Vec<SignedPayload>,
    pub signature_status: PluginSignatureStatus,
    pub approved_by: Option<UserId>,
    pub approved_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedPluginBundleDto {
    pub sha256: String,
    pub size: u64,
    /// Where members download it: `/api/plugin-bundles/<serverId>/<sha256>`.
    pub url: String,
    /// `false` when the workspace already had a bundle with this hash.
    pub created: bool,
}
