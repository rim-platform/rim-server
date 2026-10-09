use std::sync::{Arc, RwLock};

use rim_identity::SignedStatement;

use crate::descriptor::ServerDescriptor;
use crate::http_sig::KeyId;
use crate::key_log::ServerKeyState;
use crate::signer::ServerSigner;

/// This server's identity: its head server key, the verified key log and the signed
/// descriptor it publishes.
#[derive(Debug)]
pub struct ServerIdentity {
    pub signer: ServerSigner,
    pub state: ServerKeyState,
    pub log: Vec<SignedStatement>,
    pub descriptor: SignedStatement,
    pub descriptor_content: ServerDescriptor,
}

impl ServerIdentity {
    pub fn server_id(&self) -> &str {
        &self.state.server_id
    }

    /// The `keyid` this server signs with: its id and head seq.
    pub fn key_id(&self) -> KeyId {
        KeyId {
            server_id: self.state.server_id.clone(),
            seq: self.state.seq,
        }
    }
}

/// The current identity, shared by everything that signs. Replaced as a whole after a key
/// rotation or a descriptor change, so a reader never sees a key with another key's log.
#[derive(Debug, Clone)]
pub struct SharedIdentity(Arc<RwLock<Arc<ServerIdentity>>>);

impl SharedIdentity {
    pub fn new(identity: ServerIdentity) -> Self {
        Self(Arc::new(RwLock::new(Arc::new(identity))))
    }

    pub fn current(&self) -> Arc<ServerIdentity> {
        match self.0.read() {
            Ok(guard) => Arc::clone(&guard),
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    pub fn replace(&self, identity: ServerIdentity) {
        let next = Arc::new(identity);
        match self.0.write() {
            Ok(mut guard) => *guard = next,
            Err(poisoned) => *poisoned.into_inner() = next,
        }
    }
}
