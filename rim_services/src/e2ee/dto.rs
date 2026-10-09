use chrono::{DateTime, Utc};
use rim_core::conversation::model::ConversationId;
use rim_core::e2ee::model::{MlsCommitId, MlsWelcomeId};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsKeyPackageStatusDto {
    pub cipher_suite: i32,
    pub available: i64,
    pub last_resort_not_after: Option<DateTime<Utc>>,
    pub oldest_not_after: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedKeyPackageDto {
    pub user_id: UserId,
    pub principal_id: String,
    pub device_id: Uuid,
    pub key_package: Vec<u8>,
    pub key_package_ref: Vec<u8>,
    pub last_resort: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsCommitDto {
    pub id: MlsCommitId,
    pub conversation_id: ConversationId,
    pub incarnation: i32,
    /// The epoch the Commit was created in.
    pub epoch: i64,
    pub sender_principal_id: Option<String>,
    pub sender_device_id: Option<Uuid>,
    pub message: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsCommitPageDto {
    pub commits: Vec<MlsCommitDto>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsWelcomeDto {
    pub id: MlsWelcomeId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub incarnation: i32,
    pub epoch: i64,
    pub message: Vec<u8>,
    pub expires_at: DateTime<Utc>,
}

/// A leaf of the group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsDeviceDto {
    pub user_id: UserId,
    pub principal_id: String,
    pub device_id: Uuid,
    pub added_epoch: i64,
}

/// A device that belongs in the group but is not in it yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsPendingDeviceDto {
    pub user_id: UserId,
    pub principal_id: String,
    pub device_id: Uuid,
    pub has_key_package: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsReaddRequestDto {
    pub principal_id: String,
    pub device_id: Uuid,
    pub payload: String,
    pub signature: String,
    pub created_at: DateTime<Utc>,
}

/// A conversation's current group and the changes it is waiting for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsGroupDto {
    pub group_id: Vec<u8>,
    pub incarnation: i32,
    pub cipher_suite: i32,
    /// The epoch the next Commit must be created in.
    pub epoch: i64,
    pub devices: Vec<MlsDeviceDto>,
    pub pending_adds: Vec<MlsPendingDeviceDto>,
    pub pending_removes: Vec<MlsDeviceDto>,
    pub readd_requests: Vec<MlsReaddRequestDto>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalDeviceDto {
    pub device_id: Uuid,
    pub name: String,
    pub cert_payload: String,
    pub cert_signature: String,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revocation_payload: Option<String>,
    pub revocation_signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalDevicesDto {
    pub principal_id: String,
    pub devices: Vec<PrincipalDeviceDto>,
}
