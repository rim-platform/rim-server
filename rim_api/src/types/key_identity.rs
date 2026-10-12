//! Key identity types: key logs, device certificates, profile cards and challenge login.

use async_graphql::{Enum, ID, InputObject, SimpleObject};
use chrono::{DateTime, Utc};
use rim_core::key_identity::model::KeyLogEntryType as CoreKeyLogEntryType;
use rim_services::key_auth::command::SignedPayload;
use rim_services::key_auth::dto::{
    DeviceKeyDto, KeyChallengeDto, KeyIdentityDto, KeyLogEntryDto, ProfileCardDto,
};

/// A signed statement as the client sends it: the exact JSON text that was signed, and its
/// signature (`ed25519:<base64url>` in version 2 statements, bare base64url in version 1).
#[derive(InputObject, Clone, Debug)]
pub struct SignedStatementInput {
    pub payload: String,
    pub signature: String,
}

impl From<SignedStatementInput> for SignedPayload {
    fn from(value: SignedStatementInput) -> Self {
        Self {
            payload: value.payload,
            signature: value.signature,
        }
    }
}

/// A stored signed statement, exactly as it was signed, so the client can verify it again.
#[derive(SimpleObject, Clone, Debug)]
pub struct SignedStatement {
    pub payload: String,
    pub signature: String,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct KeyLoginChallenge {
    pub challenge_id: ID,
    /// Copy into the `login` statement's `nonce`.
    pub nonce: String,
    pub expires_at: DateTime<Utc>,
}

impl From<KeyChallengeDto> for KeyLoginChallenge {
    fn from(value: KeyChallengeDto) -> Self {
        Self {
            challenge_id: ID(value.challenge_id.to_string()),
            nonce: value.nonce,
            expires_at: value.expires_at,
        }
    }
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum KeyLogEntryKind {
    Inception,
    Rotate,
}

impl From<CoreKeyLogEntryType> for KeyLogEntryKind {
    fn from(value: CoreKeyLogEntryType) -> Self {
        match value {
            CoreKeyLogEntryType::Inception => Self::Inception,
            CoreKeyLogEntryType::Rotate => Self::Rotate,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct KeyLogEntry {
    pub seq: u32,
    pub kind: KeyLogEntryKind,
    pub statement: SignedStatement,
    pub created_at: DateTime<Utc>,
}

impl From<KeyLogEntryDto> for KeyLogEntry {
    fn from(value: KeyLogEntryDto) -> Self {
        Self {
            seq: value.seq,
            kind: value.entry_type.into(),
            statement: SignedStatement {
                payload: value.payload,
                signature: value.signature,
            },
            created_at: value.created_at,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct DeviceKey {
    /// The client-chosen device id from the certificate.
    pub device_id: ID,
    /// The device key in tagged form, `<algorithm>:<base64url>` (today always `ed25519:…`).
    pub public_key: String,
    pub name: String,
    /// Key log seq whose identity key signed the certificate.
    pub key_seq: u32,
    pub certificate: SignedStatement,
    pub added_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl From<DeviceKeyDto> for DeviceKey {
    fn from(value: DeviceKeyDto) -> Self {
        Self {
            device_id: ID(value.device_id.to_string()),
            public_key: value.public_key,
            name: value.name,
            key_seq: value.key_seq,
            certificate: SignedStatement {
                payload: value.cert_payload,
                signature: value.cert_signature,
            },
            added_at: value.added_at,
            last_used_at: value.last_used_at,
            revoked_at: value.revoked_at,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ProfileCard {
    pub version: u32,
    pub statement: SignedStatement,
    pub updated_at: DateTime<Utc>,
}

impl From<ProfileCardDto> for ProfileCard {
    fn from(value: ProfileCardDto) -> Self {
        Self {
            version: value.version,
            statement: SignedStatement {
                payload: value.payload,
                signature: value.signature,
            },
            updated_at: value.updated_at,
        }
    }
}

/// What this server holds about the signed-in account's key identity. Before the account
/// enrolls a key, `principalId` and `keySeq` are null and the lists are empty.
#[derive(SimpleObject, Clone, Debug)]
pub struct KeyIdentity {
    pub principal_id: Option<String>,
    pub key_seq: Option<u32>,
    /// Oldest entry first.
    pub key_log: Vec<KeyLogEntry>,
    /// Every device, revoked ones included, oldest first.
    pub devices: Vec<DeviceKey>,
    pub profile_card: Option<ProfileCard>,
}

impl From<KeyIdentityDto> for KeyIdentity {
    fn from(value: KeyIdentityDto) -> Self {
        Self {
            principal_id: value.principal_id,
            key_seq: value.key_seq,
            key_log: value.key_log.into_iter().map(Into::into).collect(),
            devices: value.devices.into_iter().map(Into::into).collect(),
            profile_card: value.profile_card.map(Into::into),
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct EnrollIdentityKeyInput {
    /// The whole key log, inception first.
    pub key_log: Vec<SignedStatementInput>,
    /// Certificate of the device enrolling; this session becomes its session.
    pub device: SignedStatementInput,
    /// A `login` statement signed by that device over a fresh `keyLoginChallenge`, proving the
    /// caller holds the device key.
    pub proof: SignedStatementInput,
}

/// Input for `signupWithKey`: an account created from a key identity alone, with no password
/// and no email. The tokens follow the same rules as `signupWithInvite`: exactly one in
/// invite-only mode, none in open mode. In pow mode it takes one token or `proofOfWork`.
#[derive(InputObject, Clone, Debug)]
pub struct SignupWithKeyInput {
    pub username: String,
    /// Optional display name (1–80 chars when present). Falls back to `username` when absent
    /// or blank after trim.
    pub display_name: Option<String>,
    /// The whole key log, inception first.
    pub key_log: Vec<SignedStatementInput>,
    /// Certificate of the device signing up; the new session belongs to it.
    pub device: SignedStatementInput,
    /// A `login` statement signed by that device over a fresh `keyLoginChallenge`.
    pub proof: SignedStatementInput,
    pub instance_invite_token: Option<String>,
    pub workspace_invite_token: Option<String>,
    /// In pow registration mode: an 8-byte nonce, unpadded base64url, such that
    /// `SHA-256("rim.pow.v1\0" || server id || "\0" || inception payload || nonce)` starts with
    /// the descriptor's `powBits` zero bits.
    pub proof_of_work: Option<String>,
    /// The terms of service the person accepted, as for `signupWithInvite`.
    pub accepted_terms_url: Option<String>,
}

#[derive(InputObject, Clone, Debug)]
pub struct RotateIdentityKeyInput {
    pub entry: SignedStatementInput,
    /// Device certificate under the new identity key. Required for a recovery rotation.
    pub device: Option<SignedStatementInput>,
}
