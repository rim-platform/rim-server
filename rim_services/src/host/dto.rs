use rim_core::conversation::model::ConversationId;
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::user::model::UserId;
use uuid::Uuid;

/// Someone from another server a local user adds to a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMemberRef {
    pub principal_id: String,
    /// Relays to fetch their identity from, in order.
    pub relay_urls: Vec<String>,
}

/// A request a relay forwarded under `/s2s/v1/forward/…`, after its S2S signature verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardRequest {
    pub peer_server_id: String,
    pub peer_origin: String,
    /// `Rim-Device-Request`: unpadded base64url of the payload text.
    pub device_request: Option<String>,
    /// `Rim-Device-Signature`.
    pub device_signature: Option<String>,
    pub method: String,
    /// The host-relative path with any query.
    pub path: String,
    pub body: Vec<u8>,
}

/// The user and device a forwarded request runs as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardAuth {
    pub user_id: UserId,
    pub device_key_id: DeviceKeyId,
    pub principal_id: String,
}

/// Why a forwarded request was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardRefusal {
    /// The device request is missing or malformed: `400 bad_request`.
    BadRequest(String),
    /// Unknown principal or device, revoked device, bad signature, or another host, method,
    /// path or body: `403 forbidden`.
    Forbidden(String),
    /// The relay may not carry this user's traffic: `403 not_delegated`.
    NotDelegated,
    /// The `seq` was seen or is below the window: `409 replay` with `highestSeq`.
    Replay { highest: i64 },
    /// Too many forwarded requests from the device: `429`.
    RateLimited { retry_after_seconds: u64 },
    /// The host failed: `500 internal`.
    Internal,
}

/// The outcome of checking a forwarded request. `request_payload` is the device request's
/// payload text whenever it could be decoded, so even a refusal carries a host response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardOutcome {
    pub request_payload: Option<String>,
    pub result: Result<ForwardAuth, ForwardRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamHeadDto {
    pub stream: String,
    pub seq: i64,
}

/// A claim of a remote user's key packages, sent to their relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteClaim {
    pub target_user_id: UserId,
    pub device_ids: Vec<Uuid>,
    pub conversation_id: ConversationId,
    pub incarnation: i32,
    pub claimer_principal_id: String,
    pub claimer_device_id: Uuid,
    pub cipher_suite: i32,
}

/// A key package a remote user's relay handed out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteKeyPackage {
    pub device_id: Uuid,
    pub key_package: Vec<u8>,
    pub last_resort: bool,
}

/// What a host cleanup pass deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostCleanup {
    pub outbox_rows: u64,
    pub seen_requests: u64,
}
