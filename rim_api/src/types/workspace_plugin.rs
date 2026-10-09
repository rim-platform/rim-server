//! Workspace plugin registry types.

use async_graphql::{Context, Enum, ID, InputObject, Object, Result, SimpleObject};
use chrono::{DateTime, Utc};
use rim_core::workspace_plugin::model::{
    PluginRuntimeKind as CoreRuntimeKind, PluginSource as CoreSource,
    WorkspacePluginStatus as CoreStatus,
};
use rim_services::key_auth::command::SignedPayload;
use rim_services::workspace_plugin::dto::{
    PluginSignatureStatus as DtoSignatureStatus, WorkspacePluginDto,
};

use super::{SignedStatement, SignedStatementInput, User};
use crate::resolve::ResolveExt;

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PluginRuntimeKind {
    DesktopLocalProcess,
    WebSandboxed,
}

impl From<CoreRuntimeKind> for PluginRuntimeKind {
    fn from(value: CoreRuntimeKind) -> Self {
        match value {
            CoreRuntimeKind::DesktopLocalProcess => Self::DesktopLocalProcess,
            CoreRuntimeKind::WebSandboxed => Self::WebSandboxed,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum WorkspacePluginStatus {
    Recommended,
    /// Offered to every member when the workspace opens; never installed without consent.
    Required,
}

impl From<CoreStatus> for WorkspacePluginStatus {
    fn from(value: CoreStatus) -> Self {
        match value {
            CoreStatus::Recommended => Self::Recommended,
            CoreStatus::Required => Self::Required,
        }
    }
}

impl From<WorkspacePluginStatus> for CoreStatus {
    fn from(value: WorkspacePluginStatus) -> Self {
        match value {
            WorkspacePluginStatus::Recommended => Self::Recommended,
            WorkspacePluginStatus::Required => Self::Required,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PluginSourceKind {
    Bundle,
    Git,
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PluginSignatureStatus {
    Valid,
    /// The publisher recovered their key after the statement's `keySeq`: clients refuse new
    /// installs and updates of the entry.
    PublisherRecovered,
}

impl From<DtoSignatureStatus> for PluginSignatureStatus {
    fn from(value: DtoSignatureStatus) -> Self {
        match value {
            DtoSignatureStatus::Valid => Self::Valid,
            DtoSignatureStatus::PublisherRecovered => Self::PublisherRecovered,
        }
    }
}

/// Where an entry's code comes from: the bundle fields for `BUNDLE`, the git fields for `GIT`.
/// Hashes are lowercase hex.
#[derive(SimpleObject, Clone, Debug)]
pub struct PluginSource {
    pub kind: PluginSourceKind,
    /// Where to fetch the bundle. Not signed: the hash decides whether the bytes are right.
    pub bundle_url: Option<String>,
    pub bundle_sha256: Option<String>,
    pub bundle_size: Option<u32>,
    pub git_url: Option<String>,
    pub git_commit: Option<String>,
    pub git_tree_sha256: Option<String>,
}

impl From<CoreSource> for PluginSource {
    fn from(value: CoreSource) -> Self {
        match value {
            CoreSource::Bundle { url, sha256, size } => Self {
                kind: PluginSourceKind::Bundle,
                bundle_url: Some(url),
                bundle_sha256: Some(sha256),
                bundle_size: u32::try_from(size).ok(),
                git_url: None,
                git_commit: None,
                git_tree_sha256: None,
            },
            CoreSource::Git {
                url,
                commit,
                tree_sha256,
            } => Self {
                kind: PluginSourceKind::Git,
                bundle_url: None,
                bundle_sha256: None,
                bundle_size: None,
                git_url: Some(url),
                git_commit: Some(commit),
                git_tree_sha256: Some(tree_sha256),
            },
        }
    }
}

fn signed_statement(value: SignedPayload) -> SignedStatement {
    SignedStatement {
        payload: value.payload,
        signature: value.signature,
    }
}

/// An entry of a workspace's plugin registry: one plugin pinned to one statement its
/// publisher signed. Clients verify the statement themselves before they offer to install it.
pub struct WorkspacePlugin(WorkspacePluginDto);

impl From<WorkspacePluginDto> for WorkspacePlugin {
    fn from(value: WorkspacePluginDto) -> Self {
        Self(value)
    }
}

#[Object]
impl WorkspacePlugin {
    async fn id(&self) -> ID {
        ID(self.0.id.to_string())
    }

    async fn server_id(&self) -> ID {
        ID(self.0.server_id.to_string())
    }

    async fn plugin_id(&self) -> &str {
        &self.0.plugin_id
    }

    async fn version(&self) -> &str {
        &self.0.version
    }

    async fn display_name(&self) -> &str {
        &self.0.display_name
    }

    async fn runtime_kind(&self) -> PluginRuntimeKind {
        self.0.runtime_kind.into()
    }

    async fn status(&self) -> WorkspacePluginStatus {
        self.0.status.into()
    }

    async fn source(&self) -> PluginSource {
        self.0.source.clone().into()
    }

    /// The exact text of `rim-plugin.json` inside the statement.
    async fn manifest(&self) -> &str {
        &self.0.manifest
    }

    async fn statement(&self) -> SignedStatement {
        signed_statement(self.0.statement.clone())
    }

    async fn publisher_principal_id(&self) -> &str {
        &self.0.publisher_principal_id
    }

    /// Chosen by the publisher: it proves nothing about who they are.
    async fn publisher_name(&self) -> &str {
        &self.0.publisher_name
    }

    async fn publisher_key_seq(&self) -> u32 {
        self.0.publisher_key_seq
    }

    /// This server's key log of the publisher when they are enrolled here, else the one stored
    /// with the entry.
    async fn publisher_key_log(&self) -> Vec<SignedStatement> {
        self.0
            .publisher_key_log
            .iter()
            .cloned()
            .map(signed_statement)
            .collect()
    }

    async fn signature_status(&self) -> PluginSignatureStatus {
        self.0.signature_status.into()
    }

    /// Who approved the entry last; null once that account is deleted.
    async fn approved_by(&self, ctx: &Context<'_>) -> Result<Option<User>> {
        match self.0.approved_by {
            Some(id) => ctx.load_user(&ID(id.to_string())).await,
            None => Ok(None),
        }
    }

    async fn approved_at(&self) -> DateTime<Utc> {
        self.0.approved_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.0.updated_at
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct AddWorkspacePluginInput {
    pub server_id: ID,
    pub statement: SignedStatementInput,
    pub status: WorkspacePluginStatus,
    /// Only for a bundle source; omitted, this server's copy, which must have been uploaded.
    pub bundle_url: Option<String>,
    /// Required when the publisher is not enrolled on this server.
    pub publisher_key_log: Option<Vec<SignedStatementInput>>,
}

#[derive(InputObject, Clone, Debug)]
pub struct UpdateWorkspacePluginInput {
    pub server_id: ID,
    pub plugin_id: String,
    /// A new version, or a re-signed one, of the same plugin.
    pub statement: Option<SignedStatementInput>,
    pub status: Option<WorkspacePluginStatus>,
    pub bundle_url: Option<String>,
    pub publisher_key_log: Option<Vec<SignedStatementInput>>,
}
