use rim_identity::{IdentityError, PublicKey, SignedStatement, verify_with_context};
use serde::de::DeserializeOwned;

use crate::error::StatementResult;
use crate::signer::ServerSigner;

/// The statement version server-signed statements are written and read in. Version 1 is not
/// accepted.
pub const STATEMENT_VERSION: u32 = 2;

/// A kind of server-signed statement. Each signs under its own context bytes, so a signature
/// made for one never verifies as another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerStatement {
    /// A server key log entry, by the server key or the server recovery key.
    ServerKey,
    /// The server descriptor, by the head server key.
    ServerDescriptor,
    /// A host's answer to a forwarded device request.
    HostResponse,
    /// An event a host sends to relays.
    HostEvent,
    /// A host's acknowledgement of a device revocation.
    RevocationAck,
}

impl ServerStatement {
    /// Bytes signed in front of the payload.
    pub fn context(self) -> &'static [u8] {
        match self {
            Self::ServerKey => b"rim.server-key.v1\0",
            Self::ServerDescriptor => b"rim.server-descriptor.v1\0",
            Self::HostResponse => b"rim.host-response.v1\0",
            Self::HostEvent => b"rim.host-event.v1\0",
            Self::RevocationAck => b"rim.revocation-ack.v1\0",
        }
    }

    /// Largest payload accepted, in bytes.
    pub fn max_payload_len(self) -> usize {
        match self {
            Self::HostEvent => 512 * 1024,
            _ => 16 * 1024,
        }
    }
}

/// Sign `payload` as a `kind` statement.
pub fn sign(
    signer: &ServerSigner,
    kind: ServerStatement,
    payload: impl Into<String>,
) -> SignedStatement {
    signer.sign_statement(kind.context(), payload)
}

/// Check a `kind` statement's size and signature by `key`. The signature's algorithm tag must
/// be the key's.
pub fn verify(
    kind: ServerStatement,
    key: &PublicKey,
    statement: &SignedStatement,
) -> StatementResult<()> {
    check_size(kind, &statement.payload)?;
    verify_with_context(kind.context(), key, statement)?;
    Ok(())
}

fn check_size(kind: ServerStatement, payload: &str) -> StatementResult<()> {
    let max = kind.max_payload_len();
    if payload.len() > max {
        return Err(IdentityError::TooLarge { max }.into());
    }
    Ok(())
}

/// Parse a payload after a size check. Structs passed here deny unknown fields, and serde
/// refuses duplicate fields, so each payload has one meaning.
pub fn parse_payload<T: DeserializeOwned>(
    kind: ServerStatement,
    payload: &str,
) -> StatementResult<T> {
    check_size(kind, payload)?;
    serde_json::from_str(payload).map_err(|e| IdentityError::Malformed(e.to_string()).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_verifies_only_for_its_kind() {
        let signer = ServerSigner::from_seed([5; 32]);
        let stmt = sign(&signer, ServerStatement::HostEvent, "{}");
        assert!(stmt.signature.starts_with("ed25519:"));
        assert_eq!(
            verify(ServerStatement::HostEvent, &signer.public_key(), &stmt),
            Ok(())
        );
        assert!(verify(ServerStatement::HostResponse, &signer.public_key(), &stmt).is_err());
        let other = ServerSigner::from_seed([6; 32]);
        assert!(verify(ServerStatement::HostEvent, &other.public_key(), &stmt).is_err());
    }

    #[test]
    fn oversized_payload_is_refused_before_the_signature() {
        let signer = ServerSigner::from_seed([5; 32]);
        let stmt = sign(
            &signer,
            ServerStatement::HostResponse,
            "x".repeat(16 * 1024 + 1),
        );
        assert!(matches!(
            verify(ServerStatement::HostResponse, &signer.public_key(), &stmt),
            Err(crate::error::StatementError::Identity(
                IdentityError::TooLarge { .. }
            ))
        ));
        let big_event = sign(&signer, ServerStatement::HostEvent, "x".repeat(100 * 1024));
        assert_eq!(
            verify(ServerStatement::HostEvent, &signer.public_key(), &big_event),
            Ok(())
        );
    }
}
