use std::fmt;

use ed25519_dalek::{Signer, SigningKey};
use rim_identity::{Algorithm, PublicKey, SignedStatement, sign_with_context};

/// A server's private signing key: the server key or the server recovery key.
///
/// Holds an Ed25519 seed. The algorithm is the public key's, so a signature always carries the
/// algorithm its verifier reads from the tagged key.
pub struct ServerSigner {
    key: SigningKey,
}

impl ServerSigner {
    /// The key derived from a 32-byte Ed25519 seed.
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            key: SigningKey::from_bytes(&seed),
        }
    }

    /// A new key from the operating system's random generator.
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed)?;
        Ok(Self::from_seed(seed))
    }

    /// The 32-byte seed, for the key file.
    pub fn seed(&self) -> [u8; 32] {
        self.key.to_bytes()
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey::from(&self.key)
    }

    pub fn algorithm(&self) -> Algorithm {
        Algorithm::Ed25519
    }

    /// A raw signature over `message`.
    pub fn sign_bytes(&self, message: &[u8]) -> Vec<u8> {
        self.key.sign(message).to_bytes().to_vec()
    }

    /// A version 2 statement: `payload` signed over `context || payload`, the signature tagged.
    pub fn sign_statement(&self, context: &[u8], payload: impl Into<String>) -> SignedStatement {
        sign_with_context(context, &self.key, payload)
    }
}

impl fmt::Debug for ServerSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ServerSigner({})", self.public_key())
    }
}
