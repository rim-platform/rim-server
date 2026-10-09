//! Helpers that build signed statements the way a client does, for tests in this crate and in
//! the server crates (enable the `testing` feature). Keys come from fixed seeds, so tests are
//! deterministic; never use these seeds for real keys.

pub use ed25519_dalek::SigningKey;
use uuid::Uuid;

use crate::device::RevocationSigner;
use crate::encoding::{PublicKey, principal_id_for_inception};
use crate::key_log::{KeyState, RotationSigner};
use crate::statement::{Purpose, SignedStatement, sign};

/// Timestamp written into test statements.
pub const TEST_TIME: &str = "2026-10-09T00:00:00Z";

/// A signing key from a one-byte seed.
pub fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// The public key of `key` in its wire form.
pub fn key_text(key: &SigningKey) -> String {
    PublicKey::from(key).to_string()
}

pub fn inception_payload(identity: &SigningKey, recovery: &SigningKey) -> String {
    format!(
        r#"{{"v":1,"type":"inception","seq":0,"identityKey":"{}","recoveryKey":"{}","createdAt":"{TEST_TIME}"}}"#,
        key_text(identity),
        key_text(recovery),
    )
}

/// A rotation of `state` to `identity`/`recovery`. The `prev` hash is taken from `state`.
pub fn rotation_payload(
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
        r#"{{"v":1,"type":"rotate","principalId":"{}","seq":{},"prev":"{}","identityKey":"{}","recoveryKey":"{}","signer":"{signer}","createdAt":"{TEST_TIME}"}}"#,
        state.principal_id,
        state.seq + 1,
        state.next_prev(),
        key_text(identity),
        key_text(recovery),
    )
}

pub fn device_cert_payload(
    state: &KeyState,
    device_id: Uuid,
    device: &SigningKey,
    name: &str,
) -> String {
    format!(
        r#"{{"v":1,"type":"device","principalId":"{}","deviceId":"{device_id}","deviceKey":"{}","name":{},"keySeq":{},"issuedAt":"{TEST_TIME}"}}"#,
        state.principal_id,
        key_text(device),
        serde_json::Value::from(name),
        state.seq,
    )
}

pub fn revocation_payload(state: &KeyState, device_id: Uuid, signer: RevocationSigner) -> String {
    let signer = match signer {
        RevocationSigner::Identity => "identity",
        RevocationSigner::Device => "device",
    };
    format!(
        r#"{{"v":1,"type":"device-revoke","principalId":"{}","deviceId":"{device_id}","signer":"{signer}","issuedAt":"{TEST_TIME}"}}"#,
        state.principal_id,
    )
}

pub fn profile_card_payload(
    state: &KeyState,
    version: u64,
    display_name: &str,
    relays: &[&str],
    devices: &[(Uuid, &SigningKey)],
) -> String {
    let devices: Vec<serde_json::Value> = devices
        .iter()
        .map(|(id, key)| serde_json::json!({ "deviceId": id, "deviceKey": key_text(key) }))
        .collect();
    serde_json::json!({
        "v": 1,
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
    state: &KeyState,
    device_id: Uuid,
    challenge_id: Uuid,
    nonce: &str,
    audience: &str,
) -> String {
    // Written out rather than built with `json!`, so the key order (and with it the known-answer
    // signature) does not depend on whether serde_json's `preserve_order` feature is on.
    format!(
        r#"{{"v":1,"type":"login","challengeId":"{challenge_id}","nonce":{},"audience":{},"principalId":"{}","deviceId":"{device_id}"}}"#,
        serde_json::Value::from(nonce),
        serde_json::Value::from(audience),
        state.principal_id,
    )
}

/// A person's identity and recovery keys, signing statements as their first device would.
pub struct TestIdentity {
    pub identity: SigningKey,
    pub recovery: SigningKey,
}

impl TestIdentity {
    /// Keys from `seed` (identity) and `seed + 100` (recovery).
    pub fn new(seed: u8) -> Self {
        Self {
            identity: signing_key(seed),
            recovery: signing_key(seed.wrapping_add(100)),
        }
    }

    pub fn inception(&self) -> SignedStatement {
        sign(
            Purpose::KeyLog,
            &self.identity,
            inception_payload(&self.identity, &self.recovery),
        )
    }

    pub fn principal_id(&self) -> String {
        principal_id_for_inception(&inception_payload(&self.identity, &self.recovery))
    }

    /// Rotate to the key from `new_seed`, signed by the current identity key, and keep the new
    /// key as this identity's.
    pub fn rotate_with_identity(&mut self, state: &KeyState, new_seed: u8) -> SignedStatement {
        let next = signing_key(new_seed);
        let payload = rotation_payload(state, &next, &self.recovery, RotationSigner::Identity);
        let entry = sign(Purpose::KeyLog, &self.identity, payload);
        self.identity = next;
        entry
    }

    /// Recover to the identity key from `new_seed`, signed by the recovery key (which stays),
    /// and keep the new identity key.
    pub fn recover(&mut self, state: &KeyState, new_seed: u8) -> SignedStatement {
        let next = signing_key(new_seed);
        let payload = rotation_payload(state, &next, &self.recovery, RotationSigner::Recovery);
        let entry = sign(Purpose::KeyLog, &self.recovery, payload);
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
            &self.identity,
            device_cert_payload(state, device_id, device, name),
        )
    }

    pub fn revoke_device(&self, state: &KeyState, device_id: Uuid) -> SignedStatement {
        sign(
            Purpose::DeviceRevoke,
            &self.identity,
            revocation_payload(state, device_id, RevocationSigner::Identity),
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
            &self.identity,
            profile_card_payload(state, version, display_name, relays, devices),
        )
    }
}

/// A device signing its own statements.
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
        device,
        login_payload(state, device_id, challenge_id, nonce, audience),
    )
}

/// A device retiring itself.
pub fn device_self_revoke(
    device: &SigningKey,
    state: &KeyState,
    device_id: Uuid,
) -> SignedStatement {
    sign(
        Purpose::DeviceRevoke,
        device,
        revocation_payload(state, device_id, RevocationSigner::Device),
    )
}
