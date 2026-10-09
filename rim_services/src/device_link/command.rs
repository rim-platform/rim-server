use crate::Actor;

/// A sealed message as the client sends and receives it: the 12-byte nonce and the AES-GCM
/// ciphertext with its tag appended, both unpadded base64url. The server checks only the
/// encoding and the size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedPayload {
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Debug, Clone)]
pub struct StartDeviceLinkCmd {
    pub actor: Actor,
}

/// Sent by the new device, without a session; the link id is its capability.
#[derive(Debug, Clone)]
pub struct SendDeviceLinkOfferCmd {
    pub link_id: String,
    pub offer: SealedPayload,
    /// Client IP for per-IP rate limiting (`"unknown"` when unavailable).
    pub remote_ip: String,
}

#[derive(Debug, Clone)]
pub struct GetDeviceLinkQuery {
    pub actor: Actor,
    pub link_id: String,
}

#[derive(Debug, Clone)]
pub struct SendDeviceLinkAnswerCmd {
    pub actor: Actor,
    pub link_id: String,
    pub answer: SealedPayload,
}

/// Polled by the new device, without a session.
#[derive(Debug, Clone)]
pub struct GetDeviceLinkAnswerQuery {
    pub link_id: String,
    pub remote_ip: String,
}

/// Sent by the new device right after its first `loginWithKey`, so `actor` is the account
/// that started the link.
#[derive(Debug, Clone)]
pub struct CompleteDeviceLinkCmd {
    pub actor: Actor,
    pub link_id: String,
}

/// Sent by either device. With a session of the link's user, the session rules apply; any
/// other caller, signed in or not, is treated as a holder of the link id.
#[derive(Debug, Clone)]
pub struct CancelDeviceLinkCmd {
    pub actor: Option<Actor>,
    pub link_id: String,
    pub remote_ip: String,
}
