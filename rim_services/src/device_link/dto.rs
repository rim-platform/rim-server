use chrono::{DateTime, Utc};

use crate::device_link::command::SealedPayload;

/// A link's state as the old device sees it. `Expired` is a live link past its expiry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceLinkStateDto {
    Open,
    Offered,
    Answered,
    Consumed,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedDeviceLinkDto {
    /// 16 random bytes, unpadded base64url. Only its hash is stored; it is returned once.
    pub link_id: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLinkDto {
    pub state: DeviceLinkStateDto,
    pub expires_at: DateTime<Utc>,
    /// The sealed offer while `Offered` or `Answered`.
    pub offer: Option<SealedPayload>,
}
