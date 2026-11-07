use chrono::{DateTime, Utc};
use rim_core::home_layout::model::HomeLayout;

/// A stored Home layout, as the API returns it.
#[derive(Clone, PartialEq)]
pub struct HomeLayoutDto {
    /// The layout document, `{ "version": 1, "boxes": [...] }`.
    pub layout: serde_json::Value,
    /// When the person made the edit, never later than the server's clock when it was stored.
    pub edited_at: DateTime<Utc>,
    /// When the server stored it.
    pub updated_at: DateTime<Utc>,
}

/// Leaves out the layout, so a log line or a panic message never carries it.
impl std::fmt::Debug for HomeLayoutDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HomeLayoutDto")
            .field("edited_at", &self.edited_at)
            .field("updated_at", &self.updated_at)
            .finish_non_exhaustive()
    }
}

impl From<HomeLayout> for HomeLayoutDto {
    fn from(value: HomeLayout) -> Self {
        Self {
            layout: value.layout,
            edited_at: value.edited_at,
            updated_at: value.updated_at,
        }
    }
}

/// The answer to a save: whether the new layout was kept, and the layout now stored.
#[derive(Debug, Clone, PartialEq)]
pub struct HomeLayoutSavedDto {
    /// False when the stored layout had a later `edited_at` and was kept instead.
    pub kept: bool,
    /// The layout just stored, or the stored one that was kept.
    pub layout: HomeLayoutDto,
}
