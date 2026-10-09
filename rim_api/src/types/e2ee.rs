//! End-to-end encryption types. Binary values are unpadded base64url; MLS messages are RFC 9420
//! `MLSMessage` bytes. Epochs and incarnations fit `Int`.

use async_graphql::{ID, InputObject, SimpleObject};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use rim_services::e2ee::dto::{
    ClaimedKeyPackageDto, MlsCommitDto, MlsCommitPageDto, MlsDeviceDto, MlsGroupDto,
    MlsKeyPackageStatusDto, MlsPendingDeviceDto, MlsReaddRequestDto, MlsWelcomeDto,
    PrincipalDeviceDto, PrincipalDevicesDto,
};
use rim_services::message::dto::MessageEncryptionDto;

use super::{SignedStatement, SignedStatementInput};

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// An epoch or count as `Int`; values past it never occur, so they saturate.
fn int(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// A conversation's current MLS group and the changes it waits for.
#[derive(SimpleObject, Clone, Debug)]
pub struct MlsGroup {
    pub group_id: String,
    pub incarnation: i32,
    pub cipher_suite: i32,
    /// The epoch the next Commit must be created in.
    pub epoch: i32,
    pub devices: Vec<MlsDevice>,
    /// MLS-active devices of members that are not in the group yet.
    pub pending_adds: Vec<MlsPendingDevice>,
    /// Devices in the group that no longer belong: their member left, they were revoked, or
    /// they uploaded no key package for 60 days.
    pub pending_removes: Vec<MlsDevice>,
    pub readd_requests: Vec<MlsReaddRequest>,
}

impl From<MlsGroupDto> for MlsGroup {
    fn from(value: MlsGroupDto) -> Self {
        Self {
            group_id: b64(&value.group_id),
            incarnation: value.incarnation,
            cipher_suite: value.cipher_suite,
            epoch: int(value.epoch),
            devices: value.devices.into_iter().map(Into::into).collect(),
            pending_adds: value.pending_adds.into_iter().map(Into::into).collect(),
            pending_removes: value.pending_removes.into_iter().map(Into::into).collect(),
            readd_requests: value.readd_requests.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MlsDevice {
    pub user_id: ID,
    pub principal_id: String,
    pub device_id: ID,
    pub added_epoch: i32,
}

impl From<MlsDeviceDto> for MlsDevice {
    fn from(value: MlsDeviceDto) -> Self {
        Self {
            user_id: ID(value.user_id.to_string()),
            principal_id: value.principal_id,
            device_id: ID(value.device_id.to_string()),
            added_epoch: int(value.added_epoch),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MlsPendingDevice {
    pub user_id: ID,
    pub principal_id: String,
    pub device_id: ID,
    pub has_key_package: bool,
}

impl From<MlsPendingDeviceDto> for MlsPendingDevice {
    fn from(value: MlsPendingDeviceDto) -> Self {
        Self {
            user_id: ID(value.user_id.to_string()),
            principal_id: value.principal_id,
            device_id: ID(value.device_id.to_string()),
            has_key_package: value.has_key_package,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MlsReaddRequest {
    pub principal_id: String,
    pub device_id: ID,
    /// The `mls-readd` statement as the device signed it.
    pub request: SignedStatement,
    pub created_at: DateTime<Utc>,
}

impl From<MlsReaddRequestDto> for MlsReaddRequest {
    fn from(value: MlsReaddRequestDto) -> Self {
        Self {
            principal_id: value.principal_id,
            device_id: ID(value.device_id.to_string()),
            request: SignedStatement {
                payload: value.payload,
                signature: value.signature,
            },
            created_at: value.created_at,
        }
    }
}

/// The MLS envelope of an encrypted message.
#[derive(SimpleObject, Clone, Debug)]
pub struct MessageEncryption {
    pub incarnation: i32,
    /// The epoch the message was sent in.
    pub epoch: i32,
    pub sender_device_id: ID,
    /// The `MLSMessage`; null once the message is deleted.
    pub ciphertext: Option<String>,
    pub franking_receipt: String,
    pub franking_key_id: i32,
    /// `sentAt` in microseconds since the Unix epoch, as decimal text, as the receipt covers it.
    pub sent_at_us: String,
}

impl From<MessageEncryptionDto> for MessageEncryption {
    fn from(value: MessageEncryptionDto) -> Self {
        Self {
            incarnation: value.incarnation,
            epoch: int(value.epoch),
            sender_device_id: ID(value.sender_device_id.to_string()),
            ciphertext: value.ciphertext.as_deref().map(b64),
            franking_receipt: b64(&value.franking_receipt),
            franking_key_id: value.franking_key_id,
            sent_at_us: value.sent_at_us.to_string(),
        }
    }
}

/// What the session's device has stored for one cipher suite.
#[derive(SimpleObject, Clone, Debug)]
pub struct MlsKeyPackageStatus {
    pub cipher_suite: i32,
    /// Unclaimed one-time key packages that can still be handed out (valid for 14 more days).
    pub available: i32,
    pub last_resort_not_after: Option<DateTime<Utc>>,
    /// How often the last-resort key package was handed out; past 5 the device replaces it.
    pub last_resort_claims: i32,
    /// The earliest expiry among the available one-time key packages.
    pub oldest_not_after: Option<DateTime<Utc>>,
}

impl From<MlsKeyPackageStatusDto> for MlsKeyPackageStatus {
    fn from(value: MlsKeyPackageStatusDto) -> Self {
        Self {
            cipher_suite: value.cipher_suite,
            available: int(value.available),
            last_resort_not_after: value.last_resort_not_after,
            last_resort_claims: value.last_resort_claims,
            oldest_not_after: value.oldest_not_after,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ClaimedKeyPackage {
    pub user_id: ID,
    pub principal_id: String,
    pub device_id: ID,
    pub key_package: String,
    pub key_package_ref: String,
    pub last_resort: bool,
}

impl From<ClaimedKeyPackageDto> for ClaimedKeyPackage {
    fn from(value: ClaimedKeyPackageDto) -> Self {
        Self {
            user_id: ID(value.user_id.to_string()),
            principal_id: value.principal_id,
            device_id: ID(value.device_id.to_string()),
            key_package: b64(&value.key_package),
            key_package_ref: b64(&value.key_package_ref),
            last_resort: value.last_resort,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MlsCommit {
    pub id: ID,
    pub incarnation: i32,
    /// The epoch the Commit was created in; the group moved to the next.
    pub epoch: i32,
    pub sender_principal_id: Option<String>,
    pub sender_device_id: Option<ID>,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

impl From<MlsCommitDto> for MlsCommit {
    fn from(value: MlsCommitDto) -> Self {
        Self {
            id: ID(value.id.to_string()),
            incarnation: value.incarnation,
            epoch: int(value.epoch),
            sender_principal_id: value.sender_principal_id,
            sender_device_id: value.sender_device_id.map(|id| ID(id.to_string())),
            message: b64(&value.message),
            created_at: value.created_at,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MlsCommitPage {
    pub commits: Vec<MlsCommit>,
    pub has_more: bool,
}

impl From<MlsCommitPageDto> for MlsCommitPage {
    fn from(value: MlsCommitPageDto) -> Self {
        Self {
            commits: value.commits.into_iter().map(Into::into).collect(),
            has_more: value.has_more,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MlsWelcome {
    pub id: ID,
    pub server_id: ID,
    pub conversation_id: ID,
    pub incarnation: i32,
    /// The epoch the device joins at.
    pub epoch: i32,
    pub message: String,
    pub expires_at: DateTime<Utc>,
}

impl From<MlsWelcomeDto> for MlsWelcome {
    fn from(value: MlsWelcomeDto) -> Self {
        Self {
            id: ID(value.id.to_string()),
            server_id: ID(value.server_id.to_string()),
            conversation_id: ID(value.conversation_id.to_string()),
            incarnation: value.incarnation,
            epoch: int(value.epoch),
            message: b64(&value.message),
            expires_at: value.expires_at,
        }
    }
}

/// A certified device of a principal, revoked or not.
#[derive(SimpleObject, Clone, Debug)]
pub struct PrincipalDevice {
    pub device_id: ID,
    pub name: String,
    pub certificate: SignedStatement,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revocation: Option<SignedStatement>,
}

impl From<PrincipalDeviceDto> for PrincipalDevice {
    fn from(value: PrincipalDeviceDto) -> Self {
        let revocation = match (value.revocation_payload, value.revocation_signature) {
            (Some(payload), Some(signature)) => Some(SignedStatement { payload, signature }),
            _ => None,
        };
        Self {
            device_id: ID(value.device_id.to_string()),
            name: value.name,
            certificate: SignedStatement {
                payload: value.cert_payload,
                signature: value.cert_signature,
            },
            revoked_at: value.revoked_at,
            revocation,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PrincipalDevices {
    pub principal_id: String,
    pub devices: Vec<PrincipalDevice>,
}

impl From<PrincipalDevicesDto> for PrincipalDevices {
    fn from(value: PrincipalDevicesDto) -> Self {
        Self {
            principal_id: value.principal_id,
            devices: value.devices.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct MlsDeviceRefInput {
    pub principal_id: String,
    pub device_id: ID,
}

#[derive(InputObject, Clone, Debug)]
pub struct ClaimMlsKeyPackagesInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub devices: Vec<MlsDeviceRefInput>,
}

#[derive(InputObject, Clone, Debug)]
pub struct SubmitMlsCommitInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub incarnation: i32,
    pub epoch: i32,
    /// Required for an incarnation's first Commit (epoch 0).
    pub cipher_suite: Option<i32>,
    pub commit: String,
    /// Required when the Commit adds devices.
    pub welcome: Option<String>,
}

#[derive(InputObject, Clone, Debug)]
pub struct SendEncryptedMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    /// Chosen by the client (UUID v7).
    pub message_id: ID,
    pub envelope: String,
    pub thread_root_message_id: Option<ID>,
    pub forwarded_from_message_id: Option<ID>,
}

#[derive(InputObject, Clone, Debug)]
pub struct EditEncryptedMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: ID,
    pub envelope: String,
}

#[derive(InputObject, Clone, Debug)]
pub struct RequestMlsReaddInput {
    pub server_id: ID,
    pub conversation_id: ID,
    /// The `mls-readd` statement, signed by the session's device key.
    pub request: SignedStatementInput,
}

#[derive(InputObject, Clone, Debug)]
pub struct ReportEncryptedMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: ID,
    pub reason: String,
    /// The decrypted content, exactly as committed to.
    pub content: String,
    pub franking_key: String,
    pub receipt: String,
    pub receipt_key_id: i32,
    pub sender_user_id: ID,
    pub sender_device_id: ID,
    /// Decimal microseconds.
    pub sent_at_us: String,
}
