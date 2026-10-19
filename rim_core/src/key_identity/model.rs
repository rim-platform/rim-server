//! Stored key identity records.
//!
//! Signed statements are kept exactly as received (`payload` plus `signature`), so anyone who
//! reads them back can verify them again. Verification itself lives in `rim_identity`.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::model_id;
use crate::user::model::UserId;

model_id!(DeviceKeyId);
model_id!(KeyChallengeId);

/// Kind of a key log entry, stored in `key_log_entry.entry_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyLogEntryType {
    Inception,
    Rotate,
}

impl KeyLogEntryType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inception => "inception",
            Self::Rotate => "rotate",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "inception" => Some(Self::Inception),
            "rotate" => Some(Self::Rotate),
            _ => None,
        }
    }
}

/// One entry of a principal's key log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyLogEntry {
    pub user_id: UserId,
    pub seq: u32,
    pub entry_type: KeyLogEntryType,
    pub payload: String,
    pub signature: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewKeyLogEntry {
    pub seq: u32,
    pub entry_type: KeyLogEntryType,
    pub payload: String,
    pub signature: String,
}

/// A device key certified by the principal's identity key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceKey {
    pub id: DeviceKeyId,
    pub user_id: UserId,
    /// Client-chosen id, unique per user, never reused after revocation.
    pub device_id: Uuid,
    /// The public key in its tagged text form, `<algorithm>:<base64url>` (`ed25519:…`).
    pub public_key: String,
    pub name: String,
    pub cert_payload: String,
    pub cert_signature: String,
    /// Key log seq whose identity key signed the certificate.
    pub key_seq: u32,
    pub added_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// The signed revocation, when one was given. A recovery rotation revokes every device
    /// without one.
    pub revocation_payload: Option<String>,
    pub revocation_signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDeviceKey {
    pub user_id: UserId,
    pub device_id: Uuid,
    /// Tagged text form, as on [`DeviceKey::public_key`].
    pub public_key: String,
    /// SHA-256 of `public_key`'s text (32 bytes): the per-user uniqueness key, so the index
    /// stays small whatever the key size.
    pub key_hash: Vec<u8>,
    pub name: String,
    pub cert_payload: String,
    pub cert_signature: String,
    pub key_seq: u32,
}

/// The latest profile card a user published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredProfileCard {
    pub user_id: UserId,
    pub version: u32,
    pub payload: String,
    pub signature: String,
    pub updated_at: DateTime<Utc>,
}

/// What a key challenge was issued for, stored in `key_challenge.purpose` and
/// `key_challenge.relay_server_id`. A challenge is consumed only for the purpose it was issued
/// for, so a proof over a join challenge never signs anyone in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyChallengePurpose {
    /// Signing in, signing up or enrolling a key identity on this server.
    Login,
    /// Joining a workspace hosted here through the relay with this server id.
    Join { relay_server_id: String },
}

impl KeyChallengePurpose {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Join { .. } => "join",
        }
    }

    /// The relay a join challenge is bound to; `None` for a login challenge.
    pub fn relay_server_id(&self) -> Option<&str> {
        match self {
            Self::Login => None,
            Self::Join { relay_server_id } => Some(relay_server_id),
        }
    }

    /// The purpose stored as `purpose` and `relay_server_id`; `None` for a combination that
    /// names no purpose.
    pub fn parse(purpose: &str, relay_server_id: Option<String>) -> Option<Self> {
        match (purpose, relay_server_id) {
            ("login", None) => Some(Self::Login),
            ("join", Some(relay_server_id)) => Some(Self::Join { relay_server_id }),
            _ => None,
        }
    }
}

/// A single-use key challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChallenge {
    pub id: KeyChallengeId,
    pub nonce: String,
    pub purpose: KeyChallengePurpose,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
}
