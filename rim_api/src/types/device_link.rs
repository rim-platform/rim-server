//! Device linking types (Stage 3, `docs/specs/device-linking-v1.md`).

use async_graphql::{Enum, InputObject, SimpleObject};
use chrono::{DateTime, Utc};
use rim_services::device_link::command::SealedPayload;
use rim_services::device_link::dto::{DeviceLinkDto, DeviceLinkStateDto, StartedDeviceLinkDto};

/// A sealed offer or answer: the 12-byte nonce and the AES-GCM ciphertext with its 16-byte tag
/// appended, both unpadded base64url. The server checks only the encoding and the size.
#[derive(InputObject, Clone, Debug)]
pub struct SealedMessageInput {
    pub nonce: String,
    pub ciphertext: String,
}

impl From<SealedMessageInput> for SealedPayload {
    fn from(value: SealedMessageInput) -> Self {
        Self {
            nonce: value.nonce,
            ciphertext: value.ciphertext,
        }
    }
}

/// A sealed message exactly as it was sent.
#[derive(SimpleObject, Clone, Debug)]
pub struct SealedMessage {
    pub nonce: String,
    pub ciphertext: String,
}

impl From<SealedPayload> for SealedMessage {
    fn from(value: SealedPayload) -> Self {
        Self {
            nonce: value.nonce,
            ciphertext: value.ciphertext,
        }
    }
}

/// `EXPIRED` is a link that was still open, offered or answered when it expired.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum DeviceLinkState {
    Open,
    Offered,
    Answered,
    Consumed,
    Cancelled,
    Expired,
}

impl From<DeviceLinkStateDto> for DeviceLinkState {
    fn from(value: DeviceLinkStateDto) -> Self {
        match value {
            DeviceLinkStateDto::Open => Self::Open,
            DeviceLinkStateDto::Offered => Self::Offered,
            DeviceLinkStateDto::Answered => Self::Answered,
            DeviceLinkStateDto::Consumed => Self::Consumed,
            DeviceLinkStateDto::Cancelled => Self::Cancelled,
            DeviceLinkStateDto::Expired => Self::Expired,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct StartedDeviceLink {
    /// 16 bytes, unpadded base64url. The server keeps only its hash, so it is shown once.
    pub link_id: String,
    pub expires_at: DateTime<Utc>,
}

impl From<StartedDeviceLinkDto> for StartedDeviceLink {
    fn from(value: StartedDeviceLinkDto) -> Self {
        Self {
            link_id: value.link_id,
            expires_at: value.expires_at,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct DeviceLink {
    pub state: DeviceLinkState,
    pub expires_at: DateTime<Utc>,
    /// The new device's sealed offer, while `OFFERED` or `ANSWERED`.
    pub offer: Option<SealedMessage>,
}

impl From<DeviceLinkDto> for DeviceLink {
    fn from(value: DeviceLinkDto) -> Self {
        Self {
            state: value.state.into(),
            expires_at: value.expires_at,
            offer: value.offer.map(Into::into),
        }
    }
}
