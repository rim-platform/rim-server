//! Stored key identity records (Stage 3, `docs/specs/key-identity-v1.md`).
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
    /// Unpadded base64url Ed25519 public key.
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
    pub public_key: String,
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

/// A single-use key login challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChallenge {
    pub id: KeyChallengeId,
    pub nonce: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
}
