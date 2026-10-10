//! A small MLS client on OpenMLS for e2e tests: a device's MLS keys and group state, key
//! packages as uploaded, Commits with the authenticated data the server reads, and encrypted
//! messages with franking.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::Signer as _;
use hmac::{Hmac, Mac};
use openmls::prelude::*;
use openmls_rust_crypto::OpenMlsRustCrypto;
use openmls_traits::signatures::{Signer, SignerError};
use rim_identity::testing::{SigningKey, device_mls_key};
use serde_json::{Value, json};
use sha2::Sha256;
use uuid::Uuid;

pub const SUITE: Ciphersuite = Ciphersuite::MLS_128_MLKEM768X25519_AES128GCM_SHA256_Ed25519;

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(text: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(text).expect("base64url")
}

pub fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("hmac key");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

pub struct MlsSigner(pub SigningKey);

impl Signer for MlsSigner {
    fn sign(&self, payload: &[u8]) -> Result<Vec<u8>, SignerError> {
        Ok(self.0.sign(payload).to_bytes().to_vec())
    }

    fn signature_scheme(&self) -> SignatureScheme {
        SignatureScheme::ED25519
    }
}

/// Leaf capabilities naming the suite explicitly: OpenMLS's defaults list it only when its own
/// post-quantum feature is on.
pub fn capabilities() -> Capabilities {
    Capabilities::new(
        None,
        Some(&[SUITE]),
        Some(&[ExtensionType::LastResort]),
        None,
        Some(&[CredentialType::Basic]),
    )
}

pub fn join_config() -> MlsGroupJoinConfig {
    MlsGroupJoinConfig::builder()
        .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
        .use_ratchet_tree_extension(true)
        .padding_size(256)
        .max_past_epochs(2)
        .build()
}

/// One device's MLS side: its device key, its MLS signer and its group state.
pub struct MlsDevice {
    pub device_id: Uuid,
    pub principal_id: String,
    pub device_key: SigningKey,
    pub signer: MlsSigner,
    pub provider: OpenMlsRustCrypto,
    pub group: Option<MlsGroup>,
}

impl MlsDevice {
    pub fn new(
        device_id: Uuid,
        principal_id: String,
        device_key: SigningKey,
        mls_key: SigningKey,
    ) -> Self {
        Self {
            device_id,
            principal_id,
            device_key,
            signer: MlsSigner(mls_key),
            provider: OpenMlsRustCrypto::default(),
            group: None,
        }
    }

    pub fn credential(&self) -> CredentialWithKey {
        let statement = device_mls_key(
            &self.device_key,
            &self.principal_id,
            self.device_id,
            &self.signer.0,
        );
        CredentialWithKey {
            credential: BasicCredential::new(rim_identity::mls_credential_identity(&statement))
                .into(),
            signature_key: self.signer.0.verifying_key().to_bytes().to_vec().into(),
        }
    }

    /// A key package as uploaded: an `MLSMessage`, unpadded base64url. Its private keys stay in
    /// this device's provider.
    pub fn key_package(&self, last_resort: bool) -> String {
        self.key_package_for(last_resort, 90 * 86_400)
    }

    /// A key package that expires `seconds` from now.
    pub fn key_package_for(&self, last_resort: bool, seconds: u64) -> String {
        let mut builder = KeyPackage::builder()
            .key_package_lifetime(Lifetime::new(seconds))
            .leaf_node_capabilities(capabilities());
        if last_resort {
            builder = builder.mark_as_last_resort();
        }
        let bundle = builder
            .build(SUITE, &self.provider, &self.signer, self.credential())
            .expect("key package");
        b64(&MlsMessageOut::from(bundle.key_package().clone())
            .to_bytes()
            .expect("encode"))
    }

    pub fn group(&mut self) -> &mut MlsGroup {
        self.group.as_mut().expect("group")
    }

    /// Merge this device's own Commit once the server accepted it.
    pub fn merge_pending(&mut self) {
        let group = self.group.as_mut().expect("group");
        group.merge_pending_commit(&self.provider).expect("merge");
    }

    /// An empty Commit with an update path.
    pub fn self_update(&mut self) -> String {
        let group = self.group.as_mut().expect("group");
        group.set_aad(commit_aad(&[], &[]));
        let bundle = group
            .self_update(&self.provider, &self.signer, LeafNodeParameters::default())
            .expect("self update");
        b64(&bundle.commit().to_bytes().expect("commit"))
    }

    pub fn epoch(&self) -> i64 {
        self.group.as_ref().expect("group").epoch().as_u64() as i64
    }

    /// An application message carrying `content` under franking key `fk`, its authenticated
    /// data naming `message_id` and mentioning `mentions`.
    pub fn encrypt(
        &mut self,
        message_id: Uuid,
        content: &[u8],
        fk: [u8; 32],
        mentions: &[&str],
    ) -> String {
        let commitment = hmac(&fk, &[b"rim.e2ee.franking.v1\0", content]);
        let aad = json!({
            "v": 1,
            "type": "application",
            "messageId": message_id,
            "franking": b64(&commitment),
            "mentions": mentions,
            "mentionAll": false,
        });
        let mut data = vec![1u8];
        data.extend_from_slice(&fk);
        data.extend_from_slice(content);
        let group = self.group.as_mut().expect("group");
        group.set_aad(aad.to_string().into_bytes());
        let message = group
            .create_message(&self.provider, &self.signer, &data)
            .expect("encrypt");
        b64(&message.to_bytes().expect("encode"))
    }

    /// Decrypt an application message and return its content, checking the franking
    /// commitment and the message id as a receiver does.
    pub fn decrypt(&mut self, ciphertext: &str, message_id: &str) -> (Vec<u8>, [u8; 32]) {
        let message =
            MlsMessageIn::tls_deserialize_exact_bytes(&unb64(ciphertext)).expect("decode");
        let group = self.group.as_mut().expect("group");
        let processed = group
            .process_message(
                &self.provider,
                message
                    .try_into_protocol_message()
                    .expect("protocol message"),
            )
            .expect("decrypt");
        let aad: Value = serde_json::from_slice(processed.aad()).expect("aad");
        assert_eq!(aad["messageId"], json!(message_id));
        let ProcessedMessageContent::ApplicationMessage(application) = processed.into_content()
        else {
            panic!("not an application message");
        };
        let data = application.into_bytes();
        assert_eq!(data[0], 1, "format version");
        let fk: [u8; 32] = data[1..33].try_into().expect("franking key");
        let content = data[33..].to_vec();
        let commitment = hmac(&fk, &[b"rim.e2ee.franking.v1\0", &content]);
        assert_eq!(
            aad["franking"],
            json!(b64(&commitment)),
            "the commitment matches the content"
        );
        (content, fk)
    }

    /// Process another device's Commit.
    pub fn process_commit(&mut self, commit: &str) {
        let message = MlsMessageIn::tls_deserialize_exact_bytes(&unb64(commit)).expect("decode");
        let group = self.group.as_mut().expect("group");
        let processed = group
            .process_message(
                &self.provider,
                message
                    .try_into_protocol_message()
                    .expect("protocol message"),
            )
            .expect("process commit");
        let ProcessedMessageContent::StagedCommitMessage(staged) = processed.into_content() else {
            panic!("not a Commit");
        };
        group
            .merge_staged_commit(&self.provider, *staged)
            .expect("merge");
    }

    /// Join a group from a Welcome (`MLSMessage` bytes, base64url).
    pub fn join(&mut self, welcome: &str) {
        let message = MlsMessageIn::tls_deserialize_exact_bytes(&unb64(welcome)).expect("decode");
        let MlsMessageBodyIn::Welcome(welcome) = message.extract() else {
            panic!("not a Welcome");
        };
        let staged = StagedWelcome::new_from_welcome(&self.provider, &join_config(), welcome, None)
            .expect("stage the Welcome");
        self.group = Some(staged.into_group(&self.provider).expect("join"));
    }
}

/// The authenticated data of a Commit adding and removing devices.
pub fn commit_aad(adds: &[(&MlsDevice, Vec<u8>)], removes: &[&MlsDevice]) -> Vec<u8> {
    let adds: Vec<(&str, Uuid, &[u8])> = adds
        .iter()
        .map(|(m, r)| (m.principal_id.as_str(), m.device_id, r.as_slice()))
        .collect();
    let removes: Vec<(&str, Uuid)> = removes
        .iter()
        .map(|m| (m.principal_id.as_str(), m.device_id))
        .collect();
    commit_aad_for(&adds, &removes)
}

/// The authenticated data of a Commit naming devices by principal and device id.
pub fn commit_aad_for(adds: &[(&str, Uuid, &[u8])], removes: &[(&str, Uuid)]) -> Vec<u8> {
    json!({
        "v": 1,
        "type": "commit",
        "adds": adds.iter().map(|(principal, device, r)| json!({
            "principalId": principal,
            "deviceId": device,
            "keyPackageRef": b64(r),
        })).collect::<Vec<_>>(),
        "removes": removes.iter().map(|(principal, device)| json!({
            "principalId": principal,
            "deviceId": device,
        })).collect::<Vec<_>>(),
    })
    .to_string()
    .into_bytes()
}

/// A claimed key package (base64url `MLSMessage`), validated, and its reference.
pub fn read_key_package(text: &str, provider: &OpenMlsRustCrypto) -> (KeyPackage, Vec<u8>) {
    let message = MlsMessageIn::tls_deserialize_exact_bytes(&unb64(text)).expect("decode");
    let MlsMessageBodyIn::KeyPackage(package) = message.extract() else {
        panic!("not a key package");
    };
    let package = package
        .validate(provider.crypto(), ProtocolVersion::Mls10)
        .expect("valid");
    let key_package_ref = package
        .hash_ref(provider.crypto())
        .expect("ref")
        .as_slice()
        .to_vec();
    (package, key_package_ref)
}

impl MlsDevice {
    /// Create a conversation's first group, adding the devices of `packages` (principal,
    /// device id, claimed key package). Returns the Commit and the Welcome, base64url.
    pub fn create_group(
        &mut self,
        conversation_id: Uuid,
        packages: &[(&str, Uuid, &str)],
    ) -> (String, String) {
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(SUITE)
            .capabilities(capabilities())
            .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
            .use_ratchet_tree_extension(true)
            .padding_size(256)
            .max_past_epochs(2)
            .build();
        let mut group_id = b"rim1".to_vec();
        group_id.extend_from_slice(conversation_id.as_bytes());
        group_id.extend_from_slice(&1u32.to_be_bytes());
        let mut group = MlsGroup::new_with_group_id(
            &self.provider,
            &self.signer,
            &config,
            GroupId::from_slice(&group_id),
            self.credential(),
        )
        .expect("group");
        let mut key_packages = Vec::new();
        let mut adds = Vec::new();
        for (principal, device, text) in packages {
            let (package, key_package_ref) = read_key_package(text, &self.provider);
            key_packages.push(package);
            adds.push((*principal, *device, key_package_ref));
        }
        let adds: Vec<(&str, Uuid, &[u8])> = adds
            .iter()
            .map(|(p, d, r)| (*p, *d, r.as_slice()))
            .collect();
        group.set_aad(commit_aad_for(&adds, &[]));
        let (commit, welcome, _) = group
            .add_members(&self.provider, &self.signer, &key_packages)
            .expect("add");
        self.group = Some(group);
        (
            b64(&commit.to_bytes().expect("commit")),
            b64(&welcome.to_bytes().expect("welcome")),
        )
    }
}

/// Message content, with the fields in the documented order.
pub fn content(id: Uuid, rev: u32, body: &str, mentions: &[&str]) -> Vec<u8> {
    json!({
        "v": 1,
        "type": "message",
        "id": id,
        "rev": rev,
        "threadRootId": null,
        "body": body,
        "mentions": mentions,
        "mentionAll": false,
        "replyTo": null,
        "forwardedFrom": null,
        "attachments": [],
        "linkPreviews": [],
    })
    .to_string()
    .into_bytes()
}
