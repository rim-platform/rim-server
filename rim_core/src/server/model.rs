use crate::model_id;
use chrono::{DateTime, Utc};

model_id!(ServerId);

// A workspace is stored as a server: `WorkspaceId` is the name for a `ServerId` where code
// speaks of workspaces.
pub type WorkspaceId = ServerId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub id: ServerId,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What deleting workspaces removed: how many, and the stored files of their attachments,
/// custom emoji and plugin bundles, which the caller deletes once its transaction commits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletedWorkspaces {
    pub deleted: u64,
    pub storage_keys: Vec<String>,
}
