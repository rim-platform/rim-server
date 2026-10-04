use crate::message::model::MessageId;
use crate::model_id;
use chrono::{DateTime, Utc};

model_id!(LinkPreviewId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPreview {
    pub id: LinkPreviewId,
    pub message_id: MessageId,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub site_name: Option<String>,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewLinkPreview {
    pub message_id: MessageId,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub site_name: Option<String>,
}
