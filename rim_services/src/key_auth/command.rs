use rim_core::auth::model::SessionId;

use crate::Actor;

/// A signed statement as the client sent it: the exact JSON text that was signed, and its
/// unpadded base64url Ed25519 signature. Formats: `docs/specs/key-identity-v1.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedPayload {
    pub payload: String,
    pub signature: String,
}

#[derive(Debug, Clone)]
pub struct IssueKeyChallengeCmd {
    /// Client IP for per-IP rate limiting (`"unknown"` when unavailable).
    pub remote_ip: String,
}

#[derive(Debug, Clone)]
pub struct LoginWithKeyCmd {
    /// A `login` statement signed by a device key.
    pub proof: SignedPayload,
    pub remote_ip: String,
}

#[derive(Debug, Clone)]
pub struct EnrollIdentityKeyCmd {
    pub actor: Actor,
    /// The caller's session; it is bound to the enrolled device.
    pub session_id: SessionId,
    /// The whole key log, inception first.
    pub key_log: Vec<SignedPayload>,
    /// Certificate of the device doing the enrolling.
    pub device: SignedPayload,
    /// A `login` statement signed by that device over a fresh challenge from this server, so
    /// a log and certificate copied from another server cannot be enrolled here.
    pub proof: SignedPayload,
}

#[derive(Debug, Clone)]
pub struct AddDeviceKeyCmd {
    pub actor: Actor,
    pub device: SignedPayload,
}

#[derive(Debug, Clone)]
pub struct RevokeDeviceKeyCmd {
    pub actor: Actor,
    pub revocation: SignedPayload,
}

#[derive(Debug, Clone)]
pub struct RotateIdentityKeyCmd {
    /// Required for a rotation signed by the identity key; a rotation signed by the recovery key
    /// works without a session, because its signer may have lost every device.
    pub actor: Option<Actor>,
    pub entry: SignedPayload,
    /// A device certificate under the new identity key. Required for a recovery rotation, which
    /// revokes every existing device.
    pub device: Option<SignedPayload>,
    pub remote_ip: String,
}

#[derive(Debug, Clone)]
pub struct PublishProfileCardCmd {
    pub actor: Actor,
    pub card: SignedPayload,
}

#[derive(Debug, Clone)]
pub struct GetMyKeyIdentityQuery {
    pub actor: Actor,
}
