use chrono::{DateTime, Utc};
use rim_core::link_preview::model::{LinkPreview, LinkPreviewId};
use rim_core::message::model::MessageId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LinkPreviewDto {
    pub id: LinkPreviewId,
    pub message_id: MessageId,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub site_name: Option<String>,
    pub fetched_at: DateTime<Utc>,
}

impl From<LinkPreview> for LinkPreviewDto {
    fn from(lp: LinkPreview) -> Self {
        Self {
            id: lp.id,
            message_id: lp.message_id,
            url: lp.url,
            title: lp.title,
            description: lp.description,
            image_url: lp.image_url,
            site_name: lp.site_name,
            fetched_at: lp.fetched_at,
        }
    }
}
