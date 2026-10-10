//! Stored host records: people from other servers, event streams, the outbox per relay, the
//! `seq` window of forwarded device requests, and the delegations relays hold.

use chrono::{DateTime, Utc};

use crate::key_identity::model::DeviceKeyId;
use crate::server::model::ServerId;
use crate::server_member::model::ServerMemberRole;
use crate::user::model::UserId;

/// Whether a `user` row is someone whose relay is this server or elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserKind {
    Local,
    Remote,
}

impl UserKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "local" => Some(Self::Local),
            "remote" => Some(Self::Remote),
            _ => None,
        }
    }
}

/// A person from another server, as a host first stores them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRemoteUser {
    pub principal_id: String,
    /// The log head's identity key, tagged.
    pub identity_key: String,
    pub username: String,
    pub display_name: String,
    pub home_relay: Option<String>,
}

/// A remote member of a space and what the outbox writer needs to address them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMember {
    pub user_id: UserId,
    pub principal_id: String,
    pub role: ServerMemberRole,
    pub home_relay: Option<String>,
    /// The newest card's payload, if one is stored.
    pub card_payload: Option<String>,
}

/// One outbox row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRow {
    pub id: i64,
    pub item: String,
    pub size_bytes: i32,
}

/// How far a relay has acknowledged its outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerDelivery {
    pub delivered_through: i64,
    pub failures: i32,
    pub next_attempt_at: DateTime<Utc>,
}

/// The answer to a forwarded device request's `seq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqDecision {
    Accepted,
    /// Seen within the window, or below it; the client moves its counter above `highest`.
    Replay {
        highest: i64,
    },
}

/// A delegation a host accepted, to store. It replaces the row for the same workspace, device
/// and relay unless that one runs longer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDelegation {
    pub workspace_id: ServerId,
    pub user_id: UserId,
    pub device_key_id: DeviceKeyId,
    pub relay_server_id: String,
    pub payload: String,
    pub signature: String,
    /// When the host stops honouring it: the earlier of the signed `expiresAt` and a day after
    /// acceptance, by the host's clock.
    pub expires_at: DateTime<Utc>,
}

/// What storing a delegation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegationStored {
    /// Stored, replacing any earlier one.
    Stored { expires_at: DateTime<Utc> },
    /// An active delegation for the same workspace, device and relay runs longer and was kept.
    Kept { expires_at: DateTime<Utc> },
}

impl DelegationStored {
    pub fn expires_at(self) -> DateTime<Utc> {
        match self {
            Self::Stored { expires_at } | Self::Kept { expires_at } => expires_at,
        }
    }
}
