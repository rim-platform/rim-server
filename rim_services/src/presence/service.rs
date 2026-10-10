use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use super::types::{PresenceStatus, UserPresence};

pub trait PresenceService: Send + Sync {
    fn heartbeat(&self, user_id: UserId, workspace_id: ServerId, status: PresenceStatus);
    fn get_workspace_presence(&self, workspace_id: ServerId) -> Vec<UserPresence>;
    fn mark_offline(&self, user_id: UserId);

    /// Set a remote member's presence in `workspace_ids` as their relay reports it: online
    /// until the relay reports them offline, with no heartbeat timeout. Servers that track no
    /// remote members ignore it.
    fn set_remote(&self, _user_id: UserId, _workspace_ids: Vec<ServerId>, _online: bool) {}
}
