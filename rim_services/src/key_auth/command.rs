use rim_core::auth::model::SessionId;

use crate::Actor;
use crate::auth::command::SignupToken;
use crate::auth::dto::RegistrationMode;

/// A signed statement as the client sent it: the exact JSON text that was signed, and its
/// signature (`ed25519:<base64url>` in version 2 statements, bare base64url in version 1).
/// Formats: `docs/specs/key-identity-v1.md`.
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

/// Create an account from a key identity alone: no password, no email.
#[derive(Debug, Clone)]
pub struct SignupWithKeyCmd {
    pub username: String,
    /// Raw input; normalized as for password signups, falling back to `username`.
    pub display_name: Option<String>,
    /// The whole key log, inception first.
    pub key_log: Vec<SignedPayload>,
    /// Certificate of the device signing up; the new session is bound to it.
    pub device: SignedPayload,
    /// A `login` statement signed by that device over a fresh challenge from this server.
    pub proof: SignedPayload,
    /// The deployment's mode: invite-only requires `token`, open refuses one.
    pub registration_mode: RegistrationMode,
    pub token: Option<SignupToken>,
    /// Client IP for the open-mode signup rate limit (`"unknown"` when unavailable).
    pub remote_ip: String,
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

/// The public key log of a principal, by id. No session: a device that lost every key reads
/// it to sign a recovery rotation that extends the current head.
#[derive(Debug, Clone)]
pub struct GetKeyLogQuery {
    pub principal_id: String,
}
