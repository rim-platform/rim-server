use crate::Actor;
use crate::auth::command::SignupToken;
use crate::auth::dto::RegistrationMode;

/// A signed statement as the client sent it: the exact JSON text that was signed, and its
/// signature (`ed25519:<base64url>` in version 2 statements, bare base64url in version 1).
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
    /// The ticket `passwordLoginFinish` answered. Required for a local account; ignored for
    /// someone from another server.
    pub password_ticket: Option<String>,
    /// "Keep me signed in"; see [`SessionKeepChoice`].
    pub keep: SessionKeepChoice,
    pub remote_ip: String,
}

/// What a new key session keeps, chosen at sign-in or signup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionKeepChoice {
    /// "Keep me signed in": a persistent cookie. Otherwise a browser-session cookie and a
    /// session secret every request carries.
    pub keep_signed_in: bool,
    /// The client holds its own keep key (the desktop app), so the server makes none. Ignored
    /// when `keep_signed_in` is false.
    pub keep_key_held: bool,
}

/// Create an account from a key identity and a password, with no email.
#[derive(Debug, Clone)]
pub struct SignupWithKeyCmd {
    pub username: String,
    /// Raw input; cleaned, falling back to `username` when blank or absent.
    pub display_name: Option<String>,
    /// The whole key log, inception first.
    pub key_log: Vec<SignedPayload>,
    /// Certificate of the device signing up; the new session is bound to it.
    pub device: SignedPayload,
    /// A `login` statement signed by that device over a fresh challenge from this server.
    pub proof: SignedPayload,
    /// The OPAQUE `RegistrationUpload` of the account's password, registered for this
    /// principal with `passwordRegistrationStart`.
    pub password_registration: Vec<u8>,
    /// The deployment's mode: invite-only requires `token`, open refuses one, pow takes
    /// `token` or `proof_of_work`.
    pub registration_mode: RegistrationMode,
    pub token: Option<SignupToken>,
    /// An 8-byte nonce, unpadded base64url, whose hash over this server's id and the
    /// inception has the mode's leading zero bits.
    pub proof_of_work: Option<String>,
    /// Client IP for the open-mode signup rate limit (`"unknown"` when unavailable).
    pub remote_ip: String,
    /// The terms URL the person accepted. The service refuses the signup when the instance has
    /// terms and this is not their URL, and records it on the new account; without instance
    /// terms it is ignored.
    pub terms_accepted_url: Option<String>,
    /// The minimum age the person confirmed reaching. The service refuses the signup when the
    /// instance has a minimum age and this is not it, and records it on the new account;
    /// without one it is ignored.
    pub confirmed_minimum_age: Option<i32>,
    /// "Keep me signed in" for the new session.
    pub keep: SessionKeepChoice,
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
