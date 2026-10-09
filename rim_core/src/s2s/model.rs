//! Stored server identity and peer records. Signed statements are kept exactly as received or
//! signed; verification lives in `rim_s2s`.

use chrono::{DateTime, Utc};

use crate::key_identity::model::KeyLogEntryType;

/// A signed statement as stored: the exact payload text and its signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedRecord {
    pub payload: String,
    pub signature: String,
}

/// One entry of this server's key log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerKeyLogRecord {
    pub seq: u32,
    pub entry_type: KeyLogEntryType,
    pub statement: SignedRecord,
}

/// The descriptor this server publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerDescriptorRecord {
    pub version: u32,
    pub statement: SignedRecord,
}

/// Whether requests to and from a peer are served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerStatus {
    Active,
    /// The peer served a key log that does not extend its pin. Nothing is served until an
    /// operator resets the pin.
    Forked,
    /// Refused by an operator.
    Blocked,
}

impl PeerStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Forked => "forked",
            Self::Blocked => "blocked",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "forked" => Some(Self::Forked),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }
}

/// Who signed the rotation that made a peer's head key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerRotationSigner {
    Server,
    Recovery,
}

impl PeerRotationSigner {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Recovery => "recovery",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "server" => Some(Self::Server),
            "recovery" => Some(Self::Recovery),
            _ => None,
        }
    }
}

/// The verified state of a peer's key log and descriptor, written when it is pinned or its pin
/// moves forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerPin {
    pub server_id: String,
    pub origin: String,
    pub head_seq: u32,
    /// Tagged text of the head server key.
    pub head_key: String,
    /// The key before the head, when the head entry is a rotation.
    pub previous_key: Option<String>,
    /// When this server learned of the head rotation.
    pub rotated_at: Option<DateTime<Utc>>,
    pub rotation_signer: Option<PeerRotationSigner>,
    pub descriptor_version: u32,
    pub descriptor: SignedRecord,
    /// The whole log, inception first.
    pub log: Vec<SignedRecord>,
}

/// A pinned peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerServer {
    pub server_id: String,
    pub origin: String,
    pub head_seq: u32,
    pub head_key: String,
    pub previous_key: Option<String>,
    pub rotated_at: Option<DateTime<Utc>>,
    pub rotation_signer: Option<PeerRotationSigner>,
    pub descriptor_version: u32,
    pub descriptor: SignedRecord,
    pub status: PeerStatus,
    pub first_seen_at: DateTime<Utc>,
    pub fetched_at: DateTime<Utc>,
    pub last_request_at: Option<DateTime<Utc>>,
}
