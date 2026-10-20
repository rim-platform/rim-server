use chrono::{DateTime, Utc};

use crate::auth::model::InstanceInviteId;
use crate::server::model::ServerId;
use crate::user::model::UserId;

/// What the instance records about the default workspace a definition file describes: the
/// workspace once an owner claimed it, and the instance invite that claims it until then.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefaultWorkspaceRecord {
    /// The claimed workspace; `None` before the claim, and after the workspace was deleted.
    pub workspace_id: Option<ServerId>,
    /// The single-use invite the owner claims the workspace with; `None` once claimed.
    pub claim_invite_id: Option<InstanceInviteId>,
    /// The account that claimed the workspace.
    pub claimed_by: Option<UserId>,
    pub claimed_at: Option<DateTime<Utc>>,
}
