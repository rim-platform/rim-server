use chrono::{DateTime, Utc};
use rim_core::relay::model::{AbuseReport, AbuseReportId, DmRequestId, DmRequestStatus};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A signed statement as it travels between servers and to clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedJson {
    pub payload: String,
    pub signature: String,
}

/// This server as its users' relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayInfoDto {
    pub server_id: String,
    pub roles: Vec<String>,
    pub direct_space_id: ServerId,
}

/// A device in an identity document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDevice {
    pub device_id: Uuid,
    pub certificate: SignedJson,
    pub revocation: Option<SignedJson>,
    pub mls_active: bool,
}

/// A person's identity as their relay serves it (`GET /s2s/v1/principals/<id>`, `POST
/// /s2s/v1/identity`): key log, devices with certificates and revocations, newest card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdentityDocument {
    pub principal_id: String,
    pub key_log: Vec<SignedJson>,
    pub devices: Vec<IdentityDevice>,
    pub card: Option<SignedJson>,
}

/// An identity and the relay it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteIdentityDto {
    pub document: IdentityDocument,
    /// Server id of the relay that served it.
    pub served_by: String,
}

/// A server pinned through `remoteServer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteServerDto {
    pub server_id: String,
    pub origin: String,
    /// The discovery document as JSON text.
    pub discovery: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactDto {
    pub principal_id: String,
    pub relay_urls: Vec<String>,
    pub added_at: DateTime<Utc>,
}

/// A queued host event as a device reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayItemDto {
    pub id: i64,
    pub host_server_id: String,
    pub workspace_id: Uuid,
    pub envelope: SignedJson,
}

impl RelayItemDto {
    /// The JSON pushed on `relayStream`.
    pub fn stream_json(&self) -> String {
        serde_json::json!({
            "type": "RelayItem",
            "id": self.id.to_string(),
            "hostServerId": self.host_server_id,
            "workspaceId": self.workspace_id,
            "envelope": self.envelope,
        })
        .to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayItemPageDto {
    pub items: Vec<RelayItemDto>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmRequestDto {
    pub id: DmRequestId,
    pub from_principal_id: String,
    pub card: SignedJson,
    pub host_server_id: String,
    pub conversation_id: Uuid,
    pub held_items: Vec<RelayItemDto>,
    pub status: DmRequestStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// A queued item's host space, named in a `QueueOverflow` notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueSpace {
    pub host_server_id: String,
    pub workspace_id: Uuid,
}

/// Something a local user's devices hear about on `relayStream`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayNotice {
    /// New items may be queued for the user.
    Items { user_id: UserId },
    /// A conversation from someone outside the user's contacts was filed as a request.
    DmRequestReceived {
        user_id: UserId,
        request_id: DmRequestId,
    },
    /// Items were dropped over the user's byte cap.
    QueueOverflow {
        user_id: UserId,
        spaces: Vec<QueueSpace>,
    },
}

impl RelayNotice {
    pub fn user_id(&self) -> UserId {
        match self {
            Self::Items { user_id }
            | Self::DmRequestReceived { user_id, .. }
            | Self::QueueOverflow { user_id, .. } => *user_id,
        }
    }

    /// The JSON pushed on `relayStream`, or `None` for a wake-up that carries nothing itself.
    pub fn stream_json(&self) -> Option<String> {
        match self {
            Self::Items { .. } => None,
            Self::DmRequestReceived { request_id, .. } => Some(
                serde_json::json!({
                    "type": "DmRequestReceived",
                    "requestId": request_id.to_string(),
                })
                .to_string(),
            ),
            Self::QueueOverflow { spaces, .. } => {
                Some(serde_json::json!({ "type": "QueueOverflow", "spaces": spaces }).to_string())
            }
        }
    }
}

/// One item of `POST /s2s/v1/deliver` and `GET /s2s/v1/outbox`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliverItem {
    /// The host's outbox id, as a decimal string.
    pub id: String,
    pub envelope: SignedJson,
    pub recipients: Vec<String>,
    pub device_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverRequest {
    pub items: Vec<DeliverItem>,
}

/// An item, or one recipient of it, the relay did not take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefusedItem {
    pub id: String,
    pub reason: String,
    /// The recipient refused, when the refusal is for one of several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliverResponse {
    /// The highest item id stored or skipped as a repeat, as a decimal string.
    pub acked_through: String,
    pub refused: Vec<RefusedItem>,
}

/// `GET /s2s/v1/outbox` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxPage {
    pub items: Vec<DeliverItem>,
    pub has_more: bool,
}

/// The conversation a key package claim is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimConversation {
    pub host: String,
    pub conversation_id: Uuid,
    pub incarnation: i32,
}

/// Who claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claimer {
    pub principal_id: String,
    pub device_id: Uuid,
}

/// `POST /s2s/v1/key-packages/claim` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PeerClaimRequest {
    pub principal_id: String,
    pub devices: Vec<Uuid>,
    pub conversation: ClaimConversation,
    pub claimer: Claimer,
    /// The group's cipher suite; the preferred suite when absent.
    #[serde(default)]
    pub cipher_suite: Option<i32>,
}

/// One claimed key package; binary values are unpadded base64url.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerKeyPackage {
    pub device_id: Uuid,
    pub key_package: String,
    pub key_package_ref: String,
    pub last_resort: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerClaimResponse {
    pub key_packages: Vec<PeerKeyPackage>,
}

/// What a cleanup pass deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayCleanup {
    pub acked_items: u64,
    pub expired_items: u64,
    pub orphan_envelopes: u64,
    pub expired_requests: u64,
}

/// A principal this server refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalBlockDto {
    pub principal_id: String,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

/// What blocking a principal did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalBlockOutcome {
    /// `false` when it was blocked already; the reason is replaced.
    pub newly_blocked: bool,
    /// The account holding the principal here, if any.
    pub user_id: Option<UserId>,
    pub ended_sessions: u64,
    /// Pending DM requests from the principal, ignored for every recipient.
    pub ignored_requests: u64,
}

/// An abuse report an operator files, from the abuse address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAbuseReport {
    pub subject_principal_id: Option<String>,
    pub subject_server_id: Option<String>,
    pub reason: String,
}

/// An abuse report as an operator reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbuseReportDto {
    pub id: AbuseReportId,
    /// The local user who reported; `None` for one an operator filed.
    pub reporter_user_id: Option<UserId>,
    pub subject_principal_id: Option<String>,
    pub subject_server_id: Option<String>,
    pub request_id: Option<DmRequestId>,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

impl From<AbuseReport> for AbuseReportDto {
    fn from(r: AbuseReport) -> Self {
        Self {
            id: r.id,
            reporter_user_id: r.reporter_user_id,
            subject_principal_id: r.subject_principal_id,
            subject_server_id: r.subject_server_id,
            request_id: r.request_id,
            reason: r.reason,
            created_at: r.created_at,
            resolved_at: r.resolved_at,
        }
    }
}
