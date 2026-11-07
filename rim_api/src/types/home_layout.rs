use async_graphql::SimpleObject;
use chrono::{DateTime, Utc};
use rim_services::home_layout::dto::{HomeLayoutDto, HomeLayoutSavedDto};

/// The signed-in person's Home layout, kept on their home server.
#[derive(SimpleObject, Clone)]
pub struct HomeLayout {
    /// The layout: `{ "version": 1, "boxes": [{ "id", "kind", "width", "settings"? }] }`.
    pub layout: async_graphql::Json<serde_json::Value>,
    /// When the person made the edit, never later than the server's clock when it was stored.
    pub edited_at: DateTime<Utc>,
    /// When the server stored it.
    pub updated_at: DateTime<Utc>,
}

impl From<HomeLayoutDto> for HomeLayout {
    fn from(value: HomeLayoutDto) -> Self {
        Self {
            layout: async_graphql::Json(value.layout),
            edited_at: value.edited_at,
            updated_at: value.updated_at,
        }
    }
}

/// The answer to `saveHomeLayout`.
#[derive(SimpleObject, Clone)]
pub struct HomeLayoutSaved {
    /// True when the layout sent was stored; false when the stored layout had a later
    /// `editedAt` and was kept instead.
    pub kept: bool,
    /// The layout just stored, or the stored one that was kept.
    pub layout: HomeLayout,
}

impl From<HomeLayoutSavedDto> for HomeLayoutSaved {
    fn from(value: HomeLayoutSavedDto) -> Self {
        Self {
            kept: value.kept,
            layout: value.layout.into(),
        }
    }
}
