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
