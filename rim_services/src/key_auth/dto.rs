use chrono::{DateTime, Utc};
use rim_core::key_identity::model::{KeyChallengeId, KeyLogEntryType};
use uuid::Uuid;

/// A key login challenge to sign.
#[derive(Debug, Clone)]
pub struct KeyChallengeDto {
    pub challenge_id: KeyChallengeId,
    pub nonce: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct KeyLogEntryDto {
    pub seq: u32,
    pub entry_type: KeyLogEntryType,
    pub payload: String,
    pub signature: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct DeviceKeyDto {
    pub device_id: Uuid,
    pub public_key: String,
    pub name: String,
    pub key_seq: u32,
    pub cert_payload: String,
    pub cert_signature: String,
    pub added_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct ProfileCardDto {
    pub version: u32,
    pub payload: String,
    pub signature: String,
    pub updated_at: DateTime<Utc>,
}

/// Everything this server holds about the caller's key identity. Empty lists and `None` before
/// the account enrolls a key.
#[derive(Debug, Clone, Default)]
pub struct KeyIdentityDto {
    pub principal_id: Option<String>,
    /// Seq of the key log head.
    pub key_seq: Option<u32>,
    pub key_log: Vec<KeyLogEntryDto>,
    /// Every device, revoked ones included, oldest first.
    pub devices: Vec<DeviceKeyDto>,
    pub profile_card: Option<ProfileCardDto>,
}
