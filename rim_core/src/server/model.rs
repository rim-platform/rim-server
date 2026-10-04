use crate::model_id;
use chrono::{DateTime, Utc};

model_id!(ServerId);

// Product terminology prefers "workspace", but the persisted/core identifier still uses
// `ServerId` today. This alias makes that mapping explicit without forcing an immediate rename.
pub type WorkspaceId = ServerId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub id: ServerId,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
