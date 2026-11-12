//! One account's Home layout as stored.

use chrono::{DateTime, Utc};

use crate::user::model::UserId;

/// An account's stored Home layout: the boxes Home shows, in order, as the client sent them.
#[derive(Clone, PartialEq)]
pub struct HomeLayout {
    pub user_id: UserId,
    /// The layout document, `{ "version": 1, "boxes": [...] }`.
    pub layout: serde_json::Value,
    /// When the person made the edit, as their client said, never later than the server's clock
    /// when it was stored.
    pub edited_at: DateTime<Utc>,
    /// When the server stored it.
    pub updated_at: DateTime<Utc>,
}

/// Leaves out the layout, so a log line or a panic message never carries it.
impl std::fmt::Debug for HomeLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HomeLayout")
            .field("user_id", &self.user_id)
            .field("edited_at", &self.edited_at)
            .field("updated_at", &self.updated_at)
            .finish_non_exhaustive()
    }
}
