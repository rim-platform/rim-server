//! Workspace plugin registries: per workspace, the plugins it recommends or requires, each
//! pinned to one statement its publisher signed, and the web plugin bundles uploaded to it.

use chrono::{DateTime, Utc};

use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;

model_id!(WorkspacePluginId);

/// Most entries one workspace's registry holds.
pub const MAX_WORKSPACE_PLUGINS: u64 = 100;

/// Stored in `workspace_plugin.runtime_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginRuntimeKind {
    DesktopLocalProcess,
    WebSandboxed,
}

impl PluginRuntimeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DesktopLocalProcess => "desktop_local_process",
            Self::WebSandboxed => "web_sandboxed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "desktop_local_process" => Some(Self::DesktopLocalProcess),
            "web_sandboxed" => Some(Self::WebSandboxed),
            _ => None,
        }
    }
}

/// Stored in `workspace_plugin.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkspacePluginStatus {
    Recommended,
    /// Offered to every member when the workspace opens; never installed without consent.
    Required,
}

impl WorkspacePluginStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recommended => "recommended",
            Self::Required => "required",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "recommended" => Some(Self::Recommended),
            "required" => Some(Self::Required),
            _ => None,
        }
    }
}

/// Where an entry's code comes from. Hashes are lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    /// A bundle file. `url` says where to fetch it and is not signed; the hash decides whether
    /// the bytes are right.
    Bundle {
        url: String,
        sha256: String,
        size: u64,
    },
    Git {
        url: String,
        commit: String,
        tree_sha256: String,
    },
}

impl PluginSource {
    /// The bundle's hash, for a bundle source.
    pub fn bundle_sha256(&self) -> Option<&str> {
        match self {
            Self::Bundle { sha256, .. } => Some(sha256),
            Self::Git { .. } => None,
        }
    }
}

/// A signed statement exactly as it was signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementText {
    pub payload: String,
    pub signature: String,
}

/// A registry entry as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePlugin {
    pub id: WorkspacePluginId,
    pub server_id: ServerId,
    pub plugin_id: String,
    pub version: String,
    pub display_name: String,
    pub runtime_kind: PluginRuntimeKind,
    pub status: WorkspacePluginStatus,
    pub source: PluginSource,
    pub statement: StatementText,
    pub publisher_principal_id: String,
    pub publisher_key_seq: u32,
    /// The publisher's key log as supplied, kept only for a publisher not enrolled on this
    /// server.
    pub publisher_key_log: Option<Vec<StatementText>>,
    /// Who approved the entry last; `None` once that account is deleted.
    pub approved_by: Option<UserId>,
    pub approved_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Every column an add or update writes. `approved_at` and `updated_at` are set to now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePluginWrite {
    pub server_id: ServerId,
    pub plugin_id: String,
    pub version: String,
    pub display_name: String,
    pub runtime_kind: PluginRuntimeKind,
    pub status: WorkspacePluginStatus,
    pub source: PluginSource,
    pub statement: StatementText,
    pub publisher_principal_id: String,
    pub publisher_key_seq: u32,
    pub publisher_key_log: Option<Vec<StatementText>>,
    pub approved_by: UserId,
}

/// A bundle uploaded to a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginBundle {
    pub server_id: ServerId,
    /// SHA-256 of the bundle file, lowercase hex.
    pub sha256: String,
    pub size_bytes: u64,
    /// Where the bytes are in file storage.
    pub storage_key: String,
    /// `None` once the uploader's account is deleted.
    pub uploaded_by: Option<UserId>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPluginBundle {
    pub server_id: ServerId,
    pub sha256: String,
    pub size_bytes: u64,
    pub storage_key: String,
    pub uploaded_by: UserId,
}
