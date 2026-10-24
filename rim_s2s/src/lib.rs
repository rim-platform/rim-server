//! Server identity and the server-to-server transport, without storage or HTTP framework: the
//! server key log and server id, the signed server descriptor, server-signed statements, and
//! the HTTP message signature profile (RFC 9421 with `Content-Digest`) servers sign requests
//! and responses with.
//!
//! - [`ServerKeyState`]: a server key log verified from its inception to its head; the server
//!   id is fixed by the inception.
//! - [`ServerDescriptor`]: roles, endpoints and limits, signed by the head server key.
//! - [`statement`]: signing and verifying server-signed statements by kind.
//! - [`host`]: host events and host responses, the statements a host signs for relays and
//!   clients.
//! - [`revocation`]: a host's signed acknowledgement of a device revocation.
//! - [`http_sig`]: signature bases, signing and verifying requests and responses.
//! - [`SharedIdentity`]: this server's key, log and descriptor, shared by everything that
//!   signs.
//! - [`handle`]: handle domains and the `@<username>:<domain>` form.
//! - [`pow`]: the proof-of-work a relay in `pow` registration mode asks of key signups.

pub mod descriptor;
mod error;
pub mod handle;
pub mod host;
pub mod http_sig;
mod identity;
mod key_log;
pub mod pow;
pub mod revocation;
mod signer;
pub mod statement;
#[cfg(test)]
mod vectors;

pub use descriptor::{
    Endpoints, Limits, Registration, RegistrationMode, Role, ServerDescriptor, sign_descriptor,
    verify_descriptor,
};
pub use error::{SignatureRefusal, StatementError, StatementResult};
pub use handle::{format_handle, is_handle_domain};
pub use http_sig::KeyId;
pub use identity::{ServerIdentity, SharedIdentity};
pub use key_log::{
    LogRelation, MAX_LOG_ENTRIES, SERVER_ID_PREFIX, ServerKeyState, ServerRotationSigner,
    compare_logs, delegable_key_hashes, is_server_id, key_hash, server_id_for_inception,
    sign_inception, sign_rotation, timestamp,
};
pub use signer::ServerSigner;
pub use statement::{STATEMENT_VERSION, ServerStatement};
