use chrono::{DateTime, Utc};

use crate::Actor;

/// Save the caller's Home layout, made at `edited_at` on their device.
#[derive(Clone)]
pub struct SaveHomeLayoutCmd {
    pub actor: Actor,
    /// The layout document as the client sent it.
    pub layout: serde_json::Value,
    /// When the person made the edit, as their client says.
    pub edited_at: DateTime<Utc>,
}

/// Leaves out the layout, so a log line or a panic message never carries it.
impl std::fmt::Debug for SaveHomeLayoutCmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SaveHomeLayoutCmd")
            .field("actor", &self.actor)
            .field("edited_at", &self.edited_at)
            .finish_non_exhaustive()
    }
}
