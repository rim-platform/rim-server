//! End-to-end encryption with MLS: the key packages, groups, Commits, Welcomes and re-add
//! requests the server stores. Every MLS message is kept as the bytes the client sent; the
//! server reads only the framing.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::conversation::model::ConversationId;
use crate::key_identity::model::DeviceKeyId;
use crate::message::model::{MessageEncryption, MessageId};
use crate::message_thread::model::MessageThreadId;
use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;

model_id!(MlsKeyPackageId);
model_id!(MlsCommitId);
model_id!(MlsWelcomeId);
model_id!(MlsReaddRequestId);

/// A device as the encryption code needs it: the row, its owner, and the ids clients know it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2eeDevice {
    pub device_key_id: DeviceKeyId,
    pub user_id: UserId,
    /// The client-chosen device id.
    pub device_id: Uuid,
    /// `None` while the owner has no key identity.
    pub principal_id: Option<String>,
    /// The device key in tagged text form.
    pub public_key: String,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_key_package_upload_at: Option<DateTime<Utc>>,
}

/// A device of a conversation member, named the way clients name it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MlsDevice {
    pub device_key_id: DeviceKeyId,
    pub user_id: UserId,
    pub principal_id: String,
    pub device_id: Uuid,
}

/// A leaf of an MLS group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsGroupDevice {
    pub device: MlsDevice,
    pub added_epoch: i64,
}

/// An MLS-active device of a current member, which belongs in the conversation's group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsDesiredDevice {
    pub device: MlsDevice,
    /// The device has an unexpired one-time or last-resort key package of the asked suite.
    pub has_key_package: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMlsKeyPackage {
    pub user_id: UserId,
    pub device_key_id: DeviceKeyId,
    pub cipher_suite: i32,
    /// The OpenMLS `KeyPackageRef`.
    pub key_package_ref: Vec<u8>,
    /// The `MLSMessage` bytes carrying the key package.
    pub key_package: Vec<u8>,
    pub last_resort: bool,
    pub not_after: DateTime<Utc>,
}

/// A stored key package without its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsKeyPackage {
    pub id: MlsKeyPackageId,
    pub user_id: UserId,
    pub device_key_id: DeviceKeyId,
    pub cipher_suite: i32,
    pub key_package_ref: Vec<u8>,
    pub last_resort: bool,
    pub not_after: DateTime<Utc>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub claimed_by_device_key_id: Option<DeviceKeyId>,
    pub claimed_for_conversation_id: Option<ConversationId>,
}

/// A key package handed to a committer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedKeyPackage {
    pub id: MlsKeyPackageId,
    pub device_key_id: DeviceKeyId,
    pub key_package_ref: Vec<u8>,
    pub key_package: Vec<u8>,
    pub last_resort: bool,
}

/// What one device has stored for one cipher suite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsKeyPackageStatus {
    pub cipher_suite: i32,
    /// Unclaimed, unexpired one-time key packages.
    pub available: i64,
    pub last_resort_not_after: Option<DateTime<Utc>>,
    /// The earliest expiry among the available one-time key packages.
    pub oldest_not_after: Option<DateTime<Utc>>,
}

/// One incarnation of a conversation's MLS group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsGroup {
    pub conversation_id: ConversationId,
    pub incarnation: i32,
    pub group_id: Vec<u8>,
    pub cipher_suite: i32,
    /// The epoch the next Commit must be created in.
    pub epoch: i64,
    pub created_by_device_key_id: Option<DeviceKeyId>,
    pub created_at: DateTime<Utc>,
    pub retired_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMlsGroup {
    pub conversation_id: ConversationId,
    pub incarnation: i32,
    pub group_id: Vec<u8>,
    pub cipher_suite: i32,
    pub epoch: i64,
    pub created_by_device_key_id: Option<DeviceKeyId>,
}

/// An accepted Commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsCommit {
    pub id: MlsCommitId,
    pub conversation_id: ConversationId,
    pub incarnation: i32,
    /// The epoch the Commit was created in; the group moved to `epoch + 1`.
    pub epoch: i64,
    pub sender_user_id: Option<UserId>,
    pub sender_device_key_id: Option<DeviceKeyId>,
    pub sender_principal_id: Option<String>,
    pub sender_device_id: Option<Uuid>,
    pub message: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMlsCommit {
    pub conversation_id: ConversationId,
    pub incarnation: i32,
    pub epoch: i64,
    pub sender_user_id: UserId,
    pub sender_device_key_id: DeviceKeyId,
    pub message: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMlsWelcome {
    pub commit_id: MlsCommitId,
    pub conversation_id: ConversationId,
    pub incarnation: i32,
    /// The epoch the added devices join at.
    pub epoch: i64,
    pub message: Vec<u8>,
    pub expires_at: DateTime<Utc>,
    /// The added devices and their owners.
    pub recipients: Vec<(DeviceKeyId, UserId)>,
}

/// A Welcome waiting for one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsWelcome {
    pub id: MlsWelcomeId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub incarnation: i32,
    pub epoch: i64,
    pub message: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// A device's signed request to be removed and added again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsReaddRequest {
    pub id: MlsReaddRequestId,
    pub conversation_id: ConversationId,
    pub incarnation: i32,
    pub device: MlsDevice,
    pub payload: String,
    pub signature: String,
    pub created_at: DateTime<Utc>,
    pub fulfilled_epoch: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMlsReaddRequest {
    pub conversation_id: ConversationId,
    pub incarnation: i32,
    pub device_key_id: DeviceKeyId,
    pub payload: String,
    pub signature: String,
}

/// An encrypted message row as the sender's request creates it. The id is the client's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEncryptedMessage {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub sent_by: UserId,
    pub thread_id: Option<MessageThreadId>,
    pub forwarded_from_message_id: Option<MessageId>,
    pub sent_at: DateTime<Utc>,
    /// `ciphertext` is always set.
    pub envelope: MessageEncryption,
}

/// The server's secret for franking receipts.
#[derive(Clone, PartialEq, Eq)]
pub struct FrankingKey {
    pub id: i32,
    pub secret: Vec<u8>,
}

impl std::fmt::Debug for FrankingKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrankingKey")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// How long each kind of row is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MlsRetention {
    pub commits: chrono::Duration,
    pub claimed_key_packages: chrono::Duration,
    pub readd_requests: chrono::Duration,
    /// Counted from the report's resolution.
    pub franked_content: chrono::Duration,
}

/// How many rows a retention run deleted or cleared.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MlsPurged {
    pub commits: u64,
    pub welcomes: u64,
    pub key_packages: u64,
    pub readd_requests: u64,
    pub franked_contents: u64,
}
