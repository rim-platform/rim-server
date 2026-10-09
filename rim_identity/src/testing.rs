//! Helpers that build signed statements the way a client does, for tests in this crate and in
//! the server crates (enable the `testing` feature). Keys come from fixed seeds, so tests are
//! deterministic; never use these seeds for real keys.
//!
//! Every builder takes the statement version to write. [`TestIdentity`] writes its own
//! `version` (version 1 unless set), and the device helpers write the version of the key log
//! head they are given. Plugin manifests are always version 2.

pub use ed25519_dalek::SigningKey;
use uuid::Uuid;

use crate::device::RevocationSigner;
use crate::encoding::{PublicKey, principal_id_for_inception};
use crate::key_log::{KeyState, RotationSigner};
use crate::statement::{Purpose, SignedStatement, StatementVersion, sign};

/// Timestamp written into test statements.
pub const TEST_TIME: &str = "2026-10-09T00:00:00Z";

/// A signing key from a one-byte seed.
pub fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// The public key of `key` as a statement of `version` spells it.
pub fn key_text(version: StatementVersion, key: &SigningKey) -> String {
    PublicKey::from(key).encode_for(version)
}

pub fn inception_payload(
    version: StatementVersion,
    identity: &SigningKey,
    recovery: &SigningKey,
) -> String {
    format!(
        r#"{{"v":{},"type":"inception","seq":0,"identityKey":"{}","recoveryKey":"{}","createdAt":"{TEST_TIME}"}}"#,
        version.number(),
        key_text(version, identity),
        key_text(version, recovery),
    )
}

/// A rotation of `state` to `identity`/`recovery`. The `prev` hash is taken from `state`.
pub fn rotation_payload(
    version: StatementVersion,
    state: &KeyState,
    identity: &SigningKey,
    recovery: &SigningKey,
    signer: RotationSigner,
) -> String {
    let signer = match signer {
        RotationSigner::Identity => "identity",
        RotationSigner::Recovery => "recovery",
    };
    format!(
        r#"{{"v":{},"type":"rotate","principalId":"{}","seq":{},"prev":"{}","identityKey":"{}","recoveryKey":"{}","signer":"{signer}","createdAt":"{TEST_TIME}"}}"#,
        version.number(),
        state.principal_id,
        state.seq + 1,
        state.next_prev(),
        key_text(version, identity),
        key_text(version, recovery),
    )
}

pub fn device_cert_payload(
    version: StatementVersion,
    state: &KeyState,
    device_id: Uuid,
    device: &SigningKey,
    name: &str,
) -> String {
    format!(
        r#"{{"v":{},"type":"device","principalId":"{}","deviceId":"{device_id}","deviceKey":"{}","name":{},"keySeq":{},"issuedAt":"{TEST_TIME}"}}"#,
        version.number(),
        state.principal_id,
        key_text(version, device),
        serde_json::Value::from(name),
        state.seq,
    )
}

pub fn revocation_payload(
    version: StatementVersion,
    state: &KeyState,
    device_id: Uuid,
    signer: RevocationSigner,
) -> String {
    let signer = match signer {
        RevocationSigner::Identity => "identity",
        RevocationSigner::Device => "device",
    };
    format!(
        r#"{{"v":{},"type":"device-revoke","principalId":"{}","deviceId":"{device_id}","signer":"{signer}","issuedAt":"{TEST_TIME}"}}"#,
        version.number(),
        state.principal_id,
    )
}

pub fn profile_card_payload(
    statement_version: StatementVersion,
    state: &KeyState,
    version: u64,
    display_name: &str,
    relays: &[&str],
    devices: &[(Uuid, &SigningKey)],
) -> String {
    let devices: Vec<serde_json::Value> = devices
        .iter()
        .map(|(id, key)| serde_json::json!({ "deviceId": id, "deviceKey": key_text(statement_version, key) }))
        .collect();
    serde_json::json!({
        "v": statement_version.number(),
        "type": "profile-card",
        "principalId": state.principal_id,
        "version": version,
        "keySeq": state.seq,
        "displayName": display_name,
        "relays": relays,
        "devices": devices,
        "issuedAt": TEST_TIME,
    })
    .to_string()
}

pub fn login_payload(
    version: StatementVersion,
    state: &KeyState,
    device_id: Uuid,
    challenge_id: Uuid,
    nonce: &str,
    audience: &str,
) -> String {
    // Written out rather than built with `json!`, so the key order (and with it the known-answer
    // signature) does not depend on whether serde_json's `preserve_order` feature is on.
    format!(
        r#"{{"v":{},"type":"login","challengeId":"{challenge_id}","nonce":{},"audience":{},"principalId":"{}","deviceId":"{device_id}"}}"#,
        version.number(),
        serde_json::Value::from(nonce),
        serde_json::Value::from(audience),
        state.principal_id,
    )
}

/// A person's identity and recovery keys, signing statements as their first device would.
pub struct TestIdentity {
    pub identity: SigningKey,
    pub recovery: SigningKey,
    /// Version of the statements this identity signs next. Change it between rotations to
    /// build a mixed-version log.
    pub version: StatementVersion,
}

impl TestIdentity {
    /// Keys from `seed` (identity) and `seed + 100` (recovery), signing version 1 statements.
    pub fn new(seed: u8) -> Self {
        Self::with_version(seed, StatementVersion::V1)
    }

    /// Like [`TestIdentity::new`], signing statements of `version`.
    pub fn with_version(seed: u8, version: StatementVersion) -> Self {
        Self {
            identity: signing_key(seed),
            recovery: signing_key(seed.wrapping_add(100)),
            version,
        }
    }

    pub fn inception(&self) -> SignedStatement {
        sign(
            Purpose::KeyLog,
            self.version,
            &self.identity,
            inception_payload(self.version, &self.identity, &self.recovery),
        )
    }

    /// The principal id of [`TestIdentity::inception`]; it depends on the version signed.
    pub fn principal_id(&self) -> String {
        principal_id_for_inception(&inception_payload(
            self.version,
            &self.identity,
            &self.recovery,
        ))
    }

    /// Rotate to the key from `new_seed`, signed by the current identity key, and keep the new
    /// key as this identity's.
    pub fn rotate_with_identity(&mut self, state: &KeyState, new_seed: u8) -> SignedStatement {
        let next = signing_key(new_seed);
        let payload = rotation_payload(
            self.version,
            state,
            &next,
            &self.recovery,
            RotationSigner::Identity,
        );
        let entry = sign(Purpose::KeyLog, self.version, &self.identity, payload);
        self.identity = next;
        entry
    }

    /// Recover to the identity key from `new_seed`, signed by the recovery key (which stays),
    /// and keep the new identity key.
    pub fn recover(&mut self, state: &KeyState, new_seed: u8) -> SignedStatement {
        let next = signing_key(new_seed);
        let payload = rotation_payload(
            self.version,
            state,
            &next,
            &self.recovery,
            RotationSigner::Recovery,
        );
        let entry = sign(Purpose::KeyLog, self.version, &self.recovery, payload);
        self.identity = next;
        entry
    }

    pub fn device_cert(
        &self,
        state: &KeyState,
        device_id: Uuid,
        device: &SigningKey,
        name: &str,
    ) -> SignedStatement {
        sign(
            Purpose::Device,
            self.version,
            &self.identity,
            device_cert_payload(self.version, state, device_id, device, name),
        )
    }

    pub fn revoke_device(&self, state: &KeyState, device_id: Uuid) -> SignedStatement {
        sign(
            Purpose::DeviceRevoke,
            self.version,
            &self.identity,
            revocation_payload(self.version, state, device_id, RevocationSigner::Identity),
        )
    }

    pub fn profile_card(
        &self,
        state: &KeyState,
        version: u64,
        display_name: &str,
        relays: &[&str],
        devices: &[(Uuid, &SigningKey)],
    ) -> SignedStatement {
        sign(
            Purpose::ProfileCard,
            self.version,
            &self.identity,
            profile_card_payload(self.version, state, version, display_name, relays, devices),
        )
    }
}

/// A device signing a login proof, at the version of the key log head `state`.
pub fn device_login(
    device: &SigningKey,
    state: &KeyState,
    device_id: Uuid,
    challenge_id: Uuid,
    nonce: &str,
    audience: &str,
) -> SignedStatement {
    sign(
        Purpose::Login,
        state.version,
        device,
        login_payload(
            state.version,
            state,
            device_id,
            challenge_id,
            nonce,
            audience,
        ),
    )
}

/// A device retiring itself, at the version of the key log head `state`.
pub fn device_self_revoke(
    device: &SigningKey,
    state: &KeyState,
    device_id: Uuid,
) -> SignedStatement {
    sign(
        Purpose::DeviceRevoke,
        state.version,
        device,
        revocation_payload(state.version, state, device_id, RevocationSigner::Device),
    )
}

/// A plugin manifest statement signed by the identity key from seed 1 at `keySeq` 0, the
/// inception of [`TestIdentity::with_version`]`(1, V2)`, over the bundle
/// [`KNOWN_PLUGIN_BUNDLE`].
pub const KNOWN_PLUGIN_MANIFEST_PAYLOAD: &str = r#"{"v":2,"type":"plugin-manifest","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","keySeq":0,"publisherName":"Ada","pluginId":"dev.example.hello-web","version":"0.1.0","manifest":"{\"manifest_version\":1,\"plugin_id\":\"dev.example.hello-web\",\"display_name\":\"Hello Web\",\"version\":\"0.1.0\",\"description\":\"Says hello from a sandboxed frame.\",\"runtime_kind\":\"WebSandboxed\",\"entrypoint\":\"main.mjs\",\"commands\":[{\"id\":\"greet\",\"title\":\"Hello Web: Greet\",\"description\":\"Greets the signed-in user.\"}]}","source":{"kind":"bundle","sha256":"5b154b3eb45c3ebc44f6820ba30a389ce85b43dea34bcbf652b9b1a5416fbe85","size":1605},"issuedAt":"2026-10-09T00:00:00Z"}"#;
pub const KNOWN_PLUGIN_MANIFEST_SIGNATURE: &str = "ed25519:FRHxrYnev03jlVo484kL4PLh7GMArjEE4q1wiBNMSx7rD8CAis8YIivjSccshXkEdvYEw-wAIjzQgzDRj6kGCg";

/// A web plugin bundle of 1,605 bytes with SHA-256
/// `5b154b3eb45c3ebc44f6820ba30a389ce85b43dea34bcbf652b9b1a5416fbe85`.
pub const KNOWN_PLUGIN_BUNDLE: &[u8] = include_bytes!("testdata/hello-web.rimbundle");

/// A compact `WebSandboxed` manifest with one command.
pub fn web_manifest(plugin_id: &str, version: &str) -> String {
    format!(
        r#"{{"manifest_version":1,"plugin_id":{},"display_name":"Hello Web","version":{},"description":"Says hello from a sandboxed frame.","runtime_kind":"WebSandboxed","entrypoint":"main.mjs","commands":[{{"id":"greet","title":"Hello Web: Greet","description":"Greets the signed-in user."}}]}}"#,
        serde_json::Value::from(plugin_id),
        serde_json::Value::from(version),
    )
}

/// A `bundle` source object.
pub fn bundle_source(sha256: &str, size: u64) -> String {
    format!(r#"{{"kind":"bundle","sha256":"{sha256}","size":{size}}}"#)
}

/// A plugin manifest payload embedding `manifest`; `pluginId` and `version` are copied from it.
/// `source` is the JSON text of the source object.
pub fn plugin_manifest_payload(
    principal_id: &str,
    key_seq: u32,
    publisher_name: &str,
    manifest: &str,
    source: &str,
) -> String {
    let fields: serde_json::Value = serde_json::from_str(manifest).unwrap_or_default();
    let field = |name: &str| fields.get(name).cloned().unwrap_or_default();
    format!(
        r#"{{"v":2,"type":"plugin-manifest","principalId":"{principal_id}","keySeq":{key_seq},"publisherName":{},"pluginId":{},"version":{},"manifest":{},"source":{source},"issuedAt":"{TEST_TIME}"}}"#,
        serde_json::Value::from(publisher_name),
        field("plugin_id"),
        field("version"),
        serde_json::Value::from(manifest),
    )
}

/// Sign a plugin manifest payload with `key`.
pub fn sign_plugin_manifest(key: &SigningKey, payload: impl Into<String>) -> SignedStatement {
    sign(Purpose::PluginManifest, StatementVersion::V2, key, payload)
}

/// An `mls-key` payload certifying `mls_key` for a device.
pub fn mls_key_payload(principal_id: &str, device_id: Uuid, mls_key: &SigningKey) -> String {
    format!(
        r#"{{"v":2,"type":"mls-key","principalId":"{principal_id}","deviceId":"{device_id}","mlsKey":"{}","issuedAt":"{TEST_TIME}"}}"#,
        key_text(StatementVersion::V2, mls_key),
    )
}

/// A device certifying its MLS signature key.
pub fn device_mls_key(
    device: &SigningKey,
    principal_id: &str,
    device_id: Uuid,
    mls_key: &SigningKey,
) -> SignedStatement {
    sign(
        Purpose::MlsKey,
        StatementVersion::V2,
        device,
        mls_key_payload(principal_id, device_id, mls_key),
    )
}

/// An `mls-readd` payload.
pub fn mls_readd_payload(
    principal_id: &str,
    device_id: Uuid,
    conversation_id: Uuid,
    incarnation: u32,
    last_epoch: Option<u64>,
    nonce: &[u8],
) -> String {
    use base64::Engine;
    let last_epoch = last_epoch.map_or("null".to_string(), |e| e.to_string());
    format!(
        r#"{{"v":2,"type":"mls-readd","principalId":"{principal_id}","deviceId":"{device_id}","conversationId":"{conversation_id}","incarnation":{incarnation},"lastEpoch":{last_epoch},"nonce":"{}","issuedAt":"{TEST_TIME}"}}"#,
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce),
    )
}

/// A device asking to be added to a conversation's group again.
pub fn device_mls_readd(
    device: &SigningKey,
    principal_id: &str,
    device_id: Uuid,
    conversation_id: Uuid,
    incarnation: u32,
    last_epoch: Option<u64>,
    nonce: &[u8],
) -> SignedStatement {
    sign(
        Purpose::MlsReadd,
        StatementVersion::V2,
        device,
        mls_readd_payload(
            principal_id,
            device_id,
            conversation_id,
            incarnation,
            last_epoch,
            nonce,
        ),
    )
}
