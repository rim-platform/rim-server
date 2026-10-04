use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use super::types::{PresenceStatus, UserPresence};

pub trait PresenceService: Send + Sync {
    fn heartbeat(&self, user_id: UserId, workspace_id: ServerId, status: PresenceStatus);
    fn get_workspace_presence(&self, workspace_id: ServerId) -> Vec<UserPresence>;
    fn mark_offline(&self, user_id: UserId);
}
