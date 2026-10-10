//! Stored relay records: host events queued for local users, the request inbox, contacts and
//! blocks. Envelopes are kept exactly as the host signed them.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::key_identity::model::DeviceKeyId;
use crate::model_id;
use crate::user::model::UserId;

model_id!(DmRequestId);

/// Where a DM request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmRequestStatus {
    Pending,
    Accepted,
    Ignored,
}

impl DmRequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Ignored => "ignored",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "accepted" => Some(Self::Accepted),
            "ignored" => Some(Self::Ignored),
            _ => None,
        }
    }
}

/// A host event as received, stored once for all its recipients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRelayEnvelope {
    pub host_server_id: String,
    pub workspace_id: Uuid,
    pub stream: String,
    /// `None` on the `e` stream, whose events have no position.
    pub seq: Option<i64>,
    pub payload: String,
    pub signature: String,
}

/// One recipient's copy of an envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRelayItem {
    pub user_id: UserId,
    pub envelope_id: i64,
    /// The one device the item is for, or `None` for every device of the user.
    pub device_key_id: Option<DeviceKeyId>,
    /// The DM request holding the item until it is accepted.
    pub request_id: Option<DmRequestId>,
    pub size_bytes: i32,
    pub expires_at: DateTime<Utc>,
}

/// A queued item as a device reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayItemRecord {
    pub id: i64,
    pub host_server_id: String,
    pub workspace_id: Uuid,
    pub payload: String,
    pub signature: String,
}

/// A space whose queued items were dropped over the byte cap.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DroppedSpace {
    pub host_server_id: String,
    pub workspace_id: Uuid,
}

/// A conversation started by someone outside the recipient's contacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmRequest {
    pub id: DmRequestId,
    pub user_id: UserId,
    pub from_principal_id: String,
    /// The sender's profile card as the host sent it.
    pub card_payload: String,
    pub card_signature: String,
    pub host_server_id: String,
    pub conversation_id: Uuid,
    pub status: DmRequestStatus,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
}

/// A request to file. A conversation from a contact is filed `Accepted` at once, so every
/// conversation a relay delivers for has a row; such rows are not listed as requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDmRequest {
    pub status: DmRequestStatus,
    pub user_id: UserId,
    pub from_principal_id: String,
    pub card_payload: String,
    pub card_signature: String,
    pub host_server_id: String,
    pub conversation_id: Uuid,
    pub expires_at: DateTime<Utc>,
}

/// Someone a local user added as a contact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub principal_id: String,
    pub relay_urls: Vec<String>,
    pub added_at: DateTime<Utc>,
}

/// An identity fetched from another relay: the `principals` JSON as served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedIdentity {
    pub principal_id: String,
    /// Server id of the relay that served it.
    pub served_by: String,
    pub identity: String,
    pub fetched_at: DateTime<Utc>,
}
