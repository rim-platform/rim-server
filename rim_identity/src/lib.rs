//! Key identity statements (Stage 3, Track 7.2): the formats a person's devices sign and the
//! checks a server runs on them. Normative description: `docs/specs/key-identity-v1.md`.
//!
//! No database, network or clock: the only I/O is the OS random generator, which
//! [`device_link`] draws link secrets and nonces from. It verifies one statement at a time
//! against the state of a key log, and leaves storage, freshness and authorization to the
//! caller (`rim_infra_services::key_auth`).
//!
//! - [`KeyState`]: a key log verified from its inception entry to its head. The principal id is
//!   fixed by the inception entry and survives every rotation.
//! - [`verify_device_cert`], [`UnverifiedRevocation`]: device keys certified and revoked.
//! - [`verify_profile_card`]: the signed card with display name, relays and devices.
//! - [`UnverifiedLogin`]: the proof a device signs over a server's login challenge.
//! - [`verify_plugin_manifest`]: a publisher's signature over a plugin's manifest and source,
//!   checked against their [`KeyHistory`].
//! - [`verify_mls_key`], [`verify_mls_readd`]: a device's MLS signature key and its requests to
//!   be added to a group again, both signed by its device key.
//! - [`device_link`]: the sealed channel a new device gets its certificate through. Clients
//!   and tests use it; the server only relays the sealed bytes.

mod device;
pub mod device_link;
mod encoding;
mod error;
mod key_log;
mod login;
mod mls;
mod plugin_manifest;
mod profile_card;
mod statement;
#[cfg(test)]
mod vectors;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use device::{
    DeviceCert, DeviceRevocation, MAX_DEVICE_NAME_CHARS, RevocationSigner, UnverifiedRevocation,
    verify_device_cert,
};
pub use encoding::{
    Algorithm, PRINCIPAL_ID_PREFIX, PublicKey, is_principal_id, principal_id_for_inception,
};
pub use error::{IdentityError, IdentityResult};
pub use key_log::{KeyState, RotationSigner, entry_signer, rotation_principal_id};
pub use login::{LoginProof, UnverifiedLogin};
pub use mls::{
    MAX_MLS_CREDENTIAL_LEN, MLS_READD_NONCE_LEN, MlsKeyStatement, MlsReaddRequest,
    UnverifiedMlsKey, UnverifiedMlsReadd, mls_credential_identity, read_mls_credential,
    verify_mls_key, verify_mls_readd,
};
pub use plugin_manifest::{
    KeyHistory, KeyHistoryEntry, MAX_BUNDLE_SIZE, MAX_GIT_URL_LEN, MAX_MANIFEST_LEN,
    MAX_PLUGIN_VERSION_CHARS, MAX_PUBLISHER_NAME_CHARS, PluginManifestStatement, PluginRuntimeKind,
    PluginSource, Validity, read_plugin_manifest, verify_plugin_manifest,
};
pub use profile_card::{CardDevice, ProfileCard, verify_profile_card};
pub use statement::{
    Purpose, SignedStatement, StatementVersion, sign, sign_with_context, verify_with_context,
};
