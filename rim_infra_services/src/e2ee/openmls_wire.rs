//! [`MlsWire`] on OpenMLS with the RustCrypto provider. The server only decodes framing and
//! validates key packages (signatures and hashes); it never creates or loads a group.

use openmls::prelude::{
    BasicCredential, ContentType, DeserializeBytes, MlsMessageBodyIn, MlsMessageIn,
    OpenMlsProvider, ProtocolVersion,
};
use openmls_rust_crypto::OpenMlsRustCrypto;

use rim_services::e2ee::wire::{
    KeyPackageInfo, MlsContentType, MlsWire, MlsWireError, PrivateMessageInfo, WelcomeInfo,
};

fn refused(message: &str) -> MlsWireError {
    MlsWireError(message.to_string())
}

/// Decodes `MLSMessage` bytes with OpenMLS.
#[derive(Default)]
pub struct OpenMlsWire {
    provider: OpenMlsRustCrypto,
}

impl OpenMlsWire {
    pub fn new() -> Self {
        Self::default()
    }

    fn decode(bytes: &[u8]) -> Result<MlsMessageBodyIn, MlsWireError> {
        MlsMessageIn::tls_deserialize_exact_bytes(bytes)
            .map(MlsMessageIn::extract)
            .map_err(|_| refused("not a valid MLSMessage"))
    }
}

impl MlsWire for OpenMlsWire {
    fn read_key_package(&self, bytes: &[u8]) -> Result<KeyPackageInfo, MlsWireError> {
        let MlsMessageBodyIn::KeyPackage(key_package) = Self::decode(bytes)? else {
            return Err(refused("the MLSMessage does not carry a key package"));
        };
        let key_package = key_package
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| MlsWireError(format!("the key package does not validate: {e}")))?;
        let leaf = key_package.leaf_node();
        let credential = BasicCredential::try_from(leaf.credential().clone())
            .map_err(|_| refused("the credential is not a basic credential"))?;
        let lifetime = key_package.life_time();
        let key_package_ref = key_package
            .hash_ref(self.provider.crypto())
            .map_err(|_| refused("the key package reference cannot be computed"))?;
        Ok(KeyPackageInfo {
            cipher_suite: i32::from(u16::from(key_package.ciphersuite())),
            key_package_ref: key_package_ref.as_slice().to_vec(),
            credential_identity: credential.identity().to_vec(),
            signature_key: leaf.signature_key().as_slice().to_vec(),
            not_after: lifetime.not_after(),
            last_resort: key_package.last_resort(),
        })
    }

    fn read_private_message(&self, bytes: &[u8]) -> Result<PrivateMessageInfo, MlsWireError> {
        let MlsMessageBodyIn::PrivateMessage(message) = Self::decode(bytes)? else {
            return Err(refused("the MLSMessage is not a PrivateMessage"));
        };
        Ok(PrivateMessageInfo {
            group_id: message.group_id().as_slice().to_vec(),
            epoch: message.epoch().as_u64(),
            content_type: match message.content_type() {
                ContentType::Application => MlsContentType::Application,
                ContentType::Proposal => MlsContentType::Proposal,
                ContentType::Commit => MlsContentType::Commit,
            },
            authenticated_data: message.aad().to_vec(),
        })
    }

    fn read_welcome(&self, bytes: &[u8]) -> Result<WelcomeInfo, MlsWireError> {
        let MlsMessageBodyIn::Welcome(welcome) = Self::decode(bytes)? else {
            return Err(refused("the MLSMessage is not a Welcome"));
        };
        Ok(WelcomeInfo {
            cipher_suite: i32::from(u16::from(welcome.ciphersuite())),
            new_member_refs: welcome
                .secrets()
                .iter()
                .map(|secret| secret.new_member().as_slice().to_vec())
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer as _, SigningKey};
    use openmls::prelude::*;
    use openmls_traits::signatures::{Signer, SignerError};

    use super::*;

    const SUITE: Ciphersuite = Ciphersuite::MLS_128_MLKEM768X25519_AES128GCM_SHA256_Ed25519;

    struct Key(SigningKey);

    impl Signer for Key {
        fn sign(&self, payload: &[u8]) -> Result<Vec<u8>, SignerError> {
            Ok(self.0.sign(payload).to_bytes().to_vec())
        }

        fn signature_scheme(&self) -> SignatureScheme {
            SignatureScheme::ED25519
        }
    }

    fn credential(key: &Key, identity: &[u8]) -> CredentialWithKey {
        CredentialWithKey {
            credential: BasicCredential::new(identity.to_vec()).into(),
            signature_key: key.0.verifying_key().to_bytes().to_vec().into(),
        }
    }

    fn key_package(provider: &OpenMlsRustCrypto, key: &Key, last_resort: bool) -> KeyPackageBundle {
        let capabilities = Capabilities::new(
            None,
            Some(&[SUITE]),
            Some(&[ExtensionType::LastResort]),
            None,
            Some(&[CredentialType::Basic]),
        );
        let mut builder = KeyPackage::builder()
            .key_package_lifetime(Lifetime::new(30 * 86_400))
            .leaf_node_capabilities(capabilities);
        if last_resort {
            builder = builder.mark_as_last_resort();
        }
        builder
            .build(SUITE, provider, key, credential(key, b"identity"))
            .expect("key package")
    }

    #[test]
    fn suite_0x004f_key_packages_validate_and_decode() {
        let client = OpenMlsRustCrypto::default();
        let key = Key(SigningKey::from_bytes(&[7; 32]));
        let bundle = key_package(&client, &key, true);
        let bytes = MlsMessageOut::from(bundle.key_package().clone())
            .to_bytes()
            .expect("encode");
        let info = OpenMlsWire::new().read_key_package(&bytes).expect("valid");
        assert_eq!(info.cipher_suite, 0x004f);
        assert_eq!(info.key_package_ref.len(), 32);
        assert_eq!(info.credential_identity, b"identity");
        assert_eq!(info.signature_key, key.0.verifying_key().to_bytes());
        assert!(info.last_resort);

        // A flipped byte in the signed body fails validation or decoding.
        let mut tampered = bytes.clone();
        let middle = tampered.len() / 2;
        tampered[middle] ^= 1;
        assert!(OpenMlsWire::new().read_key_package(&tampered).is_err());
        assert!(OpenMlsWire::new().read_key_package(&bytes[1..]).is_err());
        assert!(OpenMlsWire::new().read_private_message(&bytes).is_err());
    }

    #[test]
    fn commits_welcomes_and_messages_expose_their_framing() {
        let alice_provider = OpenMlsRustCrypto::default();
        let bob_provider = OpenMlsRustCrypto::default();
        let alice = Key(SigningKey::from_bytes(&[1; 32]));
        let bob = Key(SigningKey::from_bytes(&[2; 32]));
        let bob_package = key_package(&bob_provider, &bob, false);
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(SUITE)
            .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
            .use_ratchet_tree_extension(true)
            .build();
        let mut group = MlsGroup::new_with_group_id(
            &alice_provider,
            &alice,
            &config,
            GroupId::from_slice(&[9; 24]),
            credential(&alice, b"alice"),
        )
        .expect("group");
        group.set_aad(b"commit-aad".to_vec());
        let (commit, welcome, _) = group
            .add_members(
                &alice_provider,
                &alice,
                &[bob_package.key_package().clone()],
            )
            .expect("add");
        group.merge_pending_commit(&alice_provider).expect("merge");
        group.set_aad(b"app-aad".to_vec());
        let message = group
            .create_message(&alice_provider, &alice, b"hello")
            .expect("message");

        let wire = OpenMlsWire::new();
        let commit = wire
            .read_private_message(&commit.to_bytes().unwrap())
            .expect("commit");
        assert_eq!(commit.group_id, vec![9; 24]);
        assert_eq!(commit.epoch, 0);
        assert_eq!(commit.content_type, MlsContentType::Commit);
        assert_eq!(commit.authenticated_data, b"commit-aad");

        let message = wire
            .read_private_message(&message.to_bytes().unwrap())
            .expect("message");
        assert_eq!(message.epoch, 1);
        assert_eq!(message.content_type, MlsContentType::Application);
        assert_eq!(message.authenticated_data, b"app-aad");

        let welcome_bytes = welcome.to_bytes().unwrap();
        let welcome = wire.read_welcome(&welcome_bytes).expect("welcome");
        assert_eq!(welcome.cipher_suite, 0x004f);
        let bob_ref = bob_package
            .key_package()
            .hash_ref(bob_provider.crypto())
            .unwrap();
        assert_eq!(welcome.new_member_refs, vec![bob_ref.as_slice().to_vec()]);
        assert!(wire.read_private_message(&welcome_bytes).is_err());
        assert!(wire.read_key_package(&welcome_bytes).is_err());
    }
}
