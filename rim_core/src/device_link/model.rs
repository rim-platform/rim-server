//! Device links: a signed-in device lets a new device get its own certified device key, with
//! the server relaying a sealed offer and a sealed answer it cannot read.

use chrono::{DateTime, Duration, Utc};

use crate::model_id;
use crate::user::model::UserId;

model_id!(DeviceLinkId);

/// Stored in `device_link.state`. Expiry is `expires_at`, never a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceLinkState {
    Open,
    Offered,
    Answered,
    Consumed,
    Cancelled,
}

impl DeviceLinkState {
    /// The states a link may still move on from unless it expired.
    pub const LIVE: [Self; 3] = [Self::Open, Self::Offered, Self::Answered];

    /// The value stored in `device_link.state`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Offered => "offered",
            Self::Answered => "answered",
            Self::Consumed => "consumed",
            Self::Cancelled => "cancelled",
        }
    }

    /// The state stored as `s`, if any.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            "offered" => Some(Self::Offered),
            "answered" => Some(Self::Answered),
            "consumed" => Some(Self::Consumed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Whether the state is one of [`Self::LIVE`].
    pub fn is_live(self) -> bool {
        Self::LIVE.contains(&self)
    }
}

/// A sealed message as the server stores it: decoded bytes it never interprets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedBytes {
    pub nonce: Vec<u8>,
    /// AES-GCM output with the tag appended.
    pub ciphertext: Vec<u8>,
}

/// A device link as read. The link id itself is never stored, only its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLink {
    pub id: DeviceLinkId,
    /// The account that started the link.
    pub user_id: UserId,
    pub state: DeviceLinkState,
    /// Whether `expires_at` had passed, by the database clock, when the row was read.
    pub expired: bool,
    /// The sealed offer, while `offered` or `answered`; never set on an expired row.
    pub offer: Option<SealedBytes>,
    /// The sealed answer, while `answered`; never set on an expired row.
    pub answer: Option<SealedBytes>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub offered_at: Option<DateTime<Utc>>,
    pub answered_at: Option<DateTime<Utc>>,
    /// When the link became `consumed` or `cancelled`.
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDeviceLink {
    /// SHA-256 of the 16-byte link id (32 bytes).
    pub link_hash: Vec<u8>,
    pub user_id: UserId,
    /// How long after now (database clock) the link expires.
    pub ttl: Duration,
}
