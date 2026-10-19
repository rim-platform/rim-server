use rim_core::key_identity::model::{
    DeviceKey, KeyChallenge, KeyChallengePurpose, KeyLogEntry, KeyLogEntryType, StoredProfileCard,
};
use rim_core::repository::error::{RepoError, RepoErrorKind};

use crate::{device_key, key_challenge, key_log_entry, profile_card};

fn invalid_data() -> RepoError {
    RepoError::new(RepoErrorKind::InvalidData)
}

fn to_u32(value: i32) -> Result<u32, RepoError> {
    u32::try_from(value).map_err(|_| invalid_data())
}

impl TryFrom<key_log_entry::Model> for KeyLogEntry {
    type Error = RepoError;

    fn try_from(m: key_log_entry::Model) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: m.user_id.into(),
            seq: to_u32(m.seq)?,
            entry_type: KeyLogEntryType::parse(&m.entry_type).ok_or_else(invalid_data)?,
            payload: m.payload,
            signature: m.signature,
            created_at: m.created_at.into(),
        })
    }
}

impl TryFrom<device_key::Model> for DeviceKey {
    type Error = RepoError;

    fn try_from(m: device_key::Model) -> Result<Self, Self::Error> {
        Ok(Self {
            id: m.id.into(),
            user_id: m.user_id.into(),
            device_id: m.device_id,
            public_key: m.public_key,
            name: m.name,
            cert_payload: m.cert_payload,
            cert_signature: m.cert_signature,
            key_seq: to_u32(m.key_seq)?,
            added_at: m.added_at.into(),
            last_used_at: m.last_used_at.map(Into::into),
            revoked_at: m.revoked_at.map(Into::into),
            revocation_payload: m.revocation_payload,
            revocation_signature: m.revocation_signature,
        })
    }
}

impl TryFrom<profile_card::Model> for StoredProfileCard {
    type Error = RepoError;

    fn try_from(m: profile_card::Model) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: m.user_id.into(),
            version: u32::try_from(m.version).map_err(|_| invalid_data())?,
            payload: m.payload,
            signature: m.signature,
            updated_at: m.updated_at.into(),
        })
    }
}

impl TryFrom<key_challenge::Model> for KeyChallenge {
    type Error = RepoError;

    fn try_from(m: key_challenge::Model) -> Result<Self, Self::Error> {
        Ok(Self {
            id: m.id.into(),
            nonce: m.nonce,
            purpose: KeyChallengePurpose::parse(&m.purpose, m.relay_server_id)
                .ok_or_else(invalid_data)?,
            created_at: m.created_at.into(),
            expires_at: m.expires_at.into(),
            used_at: m.used_at.map(Into::into),
        })
    }
}
