use crate::link_preview;
use rim_core::link_preview::model::LinkPreview;

impl From<link_preview::Model> for LinkPreview {
    fn from(m: link_preview::Model) -> Self {
        Self {
            id: m.id.into(),
            message_id: m.message_id.into(),
            url: m.url,
            title: m.title,
            description: m.description,
            image_url: m.image_url,
            site_name: m.site_name,
            fetched_at: m.fetched_at.into(),
        }
    }
}
