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
    /// Actual queue rows deleted from this host and workspace.
    pub removed_items: u64,
    pub host_server_id: String,
    pub workspace_id: Uuid,
}

/// Items and bytes in the relay queue, waiting for devices (`held` false) or held behind a
/// pending DM request (`held` true).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelayQueueStats {
    pub queued_items: i64,
    pub queued_bytes: i64,
    pub held_items: i64,
    pub held_bytes: i64,
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
    /// The id the host gives the user, once an event of the host on the user's own stream named
    /// it; the host's removal of that id from the conversation ends the row.
    pub host_user_id: Option<Uuid>,
}

/// A request to file. A conversation from a contact is filed `Accepted` at once, so every
/// conversation a relay delivers for has a row; such rows are not listed as requests and keep
/// no card.
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

/// Where a relay's subscription to a workspace on another host stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionStatus {
    /// A device's delegation is active at the host.
    Active,
    /// Every delegation expired; the app renews it when a device comes back.
    Lapsed,
    /// Cancelled, or refused by the host.
    Ended,
}

impl SubscriptionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Lapsed => "lapsed",
            Self::Ended => "ended",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "lapsed" => Some(Self::Lapsed),
            "ended" => Some(Self::Ended),
            _ => None,
        }
    }
}

/// A local user's subscription to a workspace on another host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaySubscription {
    pub user_id: UserId,
    pub host_server_id: String,
    pub workspace_id: Uuid,
    pub status: SubscriptionStatus,
    /// When the host stops honouring the delegation that keeps it active.
    pub expires_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// Where a revocation's delivery to one host stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanoutStatus {
    Pending,
    /// The host answered with an acknowledgement that verified.
    Acked,
    /// 24 hours passed, or the host does not know the device.
    GaveUp,
}

impl FanoutStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Acked => "acked",
            Self::GaveUp => "gave_up",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "acked" => Some(Self::Acked),
            "gave_up" => Some(Self::GaveUp),
            _ => None,
        }
    }
}

/// A revoked device's revocation on its way to one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationFanout {
    pub device_key_id: DeviceKeyId,
    pub host_server_id: String,
    pub status: FanoutStatus,
    pub attempts: i32,
    pub next_attempt_at: DateTime<Utc>,
    /// The host's signed acknowledgement, once it answered.
    pub ack_payload: Option<String>,
    pub ack_signature: Option<String>,
}

/// A fan-out due for an attempt, with what the request carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueFanout {
    pub fanout: RevocationFanout,
    pub user_id: UserId,
    pub principal_id: String,
    pub revocation_payload: String,
    pub revocation_signature: String,
    pub revoked_at: DateTime<Utc>,
}

model_id!(AbuseReportId);

/// A principal this server refuses: its sessions, signups and deliveries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalBlock {
    pub principal_id: String,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

/// An abuse report, from a user's `reportDmRequest` or filed by an operator from the abuse
/// address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbuseReport {
    pub id: AbuseReportId,
    /// The local user who reported; `None` for a report an operator filed.
    pub reporter_user_id: Option<UserId>,
    pub subject_principal_id: Option<String>,
    pub subject_server_id: Option<String>,
    pub request_id: Option<DmRequestId>,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// A report to file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAbuseReport {
    pub reporter_user_id: Option<UserId>,
    pub subject_principal_id: Option<String>,
    pub subject_server_id: Option<String>,
    pub request_id: Option<DmRequestId>,
    pub reason: String,
}
