use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::encoding::PublicKey;
use crate::error::{IdentityError, IdentityResult};
use crate::key_log::KeyState;
use crate::statement::{
    Purpose, SignedStatement, StatementVersion, check_header, check_label, parse, verify,
};

/// Longest device name, in characters.
pub const MAX_DEVICE_NAME_CHARS: usize = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawDeviceCert {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    principal_id: String,
    device_id: Uuid,
    device_key: String,
    name: String,
    key_seq: u32,
    #[allow(dead_code)]
    issued_at: DateTime<Utc>,
}

/// A device key certified by the principal's identity key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCert {
    pub principal_id: String,
    pub device_id: Uuid,
    pub device_key: PublicKey,
    /// Trimmed, 1 to [`MAX_DEVICE_NAME_CHARS`] characters.
    pub name: String,
    pub key_seq: u32,
}

/// Verify a device certificate against the head of the principal's key log.
///
/// The certificate must be signed by the current identity key and name the current `keySeq`,
/// so a key that was rotated away cannot certify new devices. A device key must differ from the
/// identity and recovery keys.
pub fn verify_device_cert(state: &KeyState, cert: &SignedStatement) -> IdentityResult<DeviceCert> {
    let raw: RawDeviceCert = parse(Purpose::Device, &cert.payload)?;
    let version = check_header(raw.v, &raw.kind, "device")?;
    if raw.principal_id != state.principal_id {
        return Err(IdentityError::PrincipalMismatch);
    }
    if raw.key_seq != state.seq {
        return Err(IdentityError::StaleKey {
            current: state.seq,
            got: raw.key_seq,
        });
    }
    let device_key = PublicKey::parse_for(version, &raw.device_key, "deviceKey")?;
    if device_key == state.identity_key || device_key == state.recovery_key {
        return Err(IdentityError::Invalid(
            "deviceKey must differ from the identity and recovery keys".to_string(),
        ));
    }
    let name = check_label(&raw.name, "name", MAX_DEVICE_NAME_CHARS)?;
    verify(Purpose::Device, version, &state.identity_key, cert)?;
    Ok(DeviceCert {
        principal_id: raw.principal_id,
        device_id: raw.device_id,
        device_key,
        name,
        key_seq: raw.key_seq,
    })
}

/// Who signed a device revocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RevocationSigner {
    /// The current identity key, revoking any device.
    Identity,
    /// The revoked device's own key, retiring itself.
    Device,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawDeviceRevocation {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    principal_id: String,
    device_id: Uuid,
    signer: RevocationSigner,
    #[allow(dead_code)]
    issued_at: DateTime<Utc>,
}

/// A device revocation whose signature has not been checked yet.
///
/// The verifier needs the revoked device's key, which it looks up by `device_id`; only
/// [`UnverifiedRevocation::verify`] turns this into a [`DeviceRevocation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedRevocation {
    pub principal_id: String,
    pub device_id: Uuid,
    pub signer: RevocationSigner,
    version: StatementVersion,
    statement: SignedStatement,
}

/// A verified device revocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRevocation {
    pub principal_id: String,
    pub device_id: Uuid,
    pub signer: RevocationSigner,
}

impl UnverifiedRevocation {
    /// Parse a revocation without checking its signature.
    pub fn read(revocation: &SignedStatement) -> IdentityResult<Self> {
        let raw: RawDeviceRevocation = parse(Purpose::DeviceRevoke, &revocation.payload)?;
        let version = check_header(raw.v, &raw.kind, "device-revoke")?;
        Ok(Self {
            principal_id: raw.principal_id,
            device_id: raw.device_id,
            signer: raw.signer,
            version,
            statement: revocation.clone(),
        })
    }

    /// Check the signature: by the current identity key, or by `device_key` (the key of the
    /// device being revoked) when the device revokes itself.
    pub fn verify(
        self,
        state: &KeyState,
        device_key: &PublicKey,
    ) -> IdentityResult<DeviceRevocation> {
        if self.principal_id != state.principal_id {
            return Err(IdentityError::PrincipalMismatch);
        }
        let key = match self.signer {
            RevocationSigner::Identity => &state.identity_key,
            RevocationSigner::Device => device_key,
        };
        verify(Purpose::DeviceRevoke, self.version, key, &self.statement)?;
        Ok(DeviceRevocation {
            principal_id: self.principal_id,
            device_id: self.device_id,
            signer: self.signer,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statement::StatementVersion::{V1, V2};
    use crate::statement::sign;
    use crate::testing::{TestIdentity, device_cert_payload, revocation_payload, signing_key};

    fn setup() -> (TestIdentity, KeyState) {
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        (id, state)
    }

    #[test]
    fn cert_signed_by_current_identity_verifies() {
        let (id, state) = setup();
        let device = signing_key(20);
        let device_id = Uuid::from_u128(1);
        let cert = id.device_cert(&state, device_id, &device, "  Laptop ");
        let verified = verify_device_cert(&state, &cert).expect("cert");
        assert_eq!(verified.device_id, device_id);
        assert_eq!(verified.device_key, PublicKey::from(&device));
        assert_eq!(verified.name, "Laptop");
        assert_eq!(verified.key_seq, 0);
    }

    #[test]
    fn cert_signed_by_device_itself_fails() {
        let (_, state) = setup();
        let device = signing_key(20);
        let payload = device_cert_payload(V1, &state, Uuid::from_u128(1), &device, "Laptop");
        let cert = sign(Purpose::Device, V1, &device, payload);
        assert_eq!(
            verify_device_cert(&state, &cert),
            Err(IdentityError::BadSignature)
        );
    }

    #[test]
    fn cert_from_rotated_away_key_is_stale() {
        let (mut id, state) = setup();
        let old_cert = id.device_cert(&state, Uuid::from_u128(1), &signing_key(20), "Laptop");
        let rotation = id.rotate_with_identity(&state, 2);
        let (next, _) = state.apply(&rotation).expect("rotate");
        assert_eq!(
            verify_device_cert(&next, &old_cert),
            Err(IdentityError::StaleKey { current: 1, got: 0 })
        );
    }

    #[test]
    fn cert_rejects_identity_key_as_device_key_and_bad_names() {
        let (id, state) = setup();
        let cert = id.device_cert(&state, Uuid::from_u128(1), &id.identity, "Laptop");
        assert!(matches!(
            verify_device_cert(&state, &cert),
            Err(IdentityError::Invalid(_))
        ));
        let cert = id.device_cert(&state, Uuid::from_u128(1), &signing_key(20), " ");
        assert!(matches!(
            verify_device_cert(&state, &cert),
            Err(IdentityError::Invalid(_))
        ));
    }

    #[test]
    fn cert_for_other_principal_fails() {
        let (id, state) = setup();
        let other = TestIdentity::new(7);
        let other_state = KeyState::from_inception(&other.inception()).expect("other");
        let cert = id.device_cert(&other_state, Uuid::from_u128(1), &signing_key(20), "x");
        assert_eq!(
            verify_device_cert(&state, &cert),
            Err(IdentityError::PrincipalMismatch)
        );
    }

    #[test]
    fn revocation_by_identity_or_by_the_device_itself() {
        let (id, state) = setup();
        let device = signing_key(20);
        let device_key = PublicKey::from(&device);
        let device_id = Uuid::from_u128(1);

        let by_identity = id.revoke_device(&state, device_id);
        let read = UnverifiedRevocation::read(&by_identity).expect("read");
        assert_eq!(read.device_id, device_id);
        assert_eq!(read.signer, RevocationSigner::Identity);
        assert!(read.verify(&state, &device_key).is_ok());

        let payload = revocation_payload(V1, &state, device_id, RevocationSigner::Device);
        let by_device = sign(Purpose::DeviceRevoke, V1, &device, payload);
        let verified = UnverifiedRevocation::read(&by_device)
            .expect("read")
            .verify(&state, &device_key)
            .expect("verify");
        assert_eq!(verified.signer, RevocationSigner::Device);
    }

    #[test]
    fn revocation_claiming_device_signer_but_signed_by_other_device_fails() {
        let (_, state) = setup();
        let payload = revocation_payload(V1, &state, Uuid::from_u128(1), RevocationSigner::Device);
        let forged = sign(Purpose::DeviceRevoke, V1, &signing_key(21), payload);
        let result = UnverifiedRevocation::read(&forged)
            .expect("read")
            .verify(&state, &PublicKey::from(&signing_key(20)));
        assert_eq!(result, Err(IdentityError::BadSignature));
    }

    #[test]
    fn version_2_cert_and_revocation_verify_under_a_version_1_log() {
        let (mut id, state) = setup();
        id.version = V2;
        let device = signing_key(20);
        let cert = id.device_cert(&state, Uuid::from_u128(1), &device, "Laptop");
        assert!(cert.payload.contains(r#""deviceKey":"ed25519:"#));
        let verified = verify_device_cert(&state, &cert).expect("v2 cert");
        assert_eq!(verified.device_key, PublicKey::from(&device));

        let revocation = id.revoke_device(&state, Uuid::from_u128(1));
        assert!(revocation.signature.starts_with("ed25519:"));
        UnverifiedRevocation::read(&revocation)
            .expect("read")
            .verify(&state, &verified.device_key)
            .expect("v2 revocation");
    }

    #[test]
    fn version_2_cert_with_bare_signature_fails() {
        let (_, state) = setup();
        let id = TestIdentity::with_version(1, V2);
        let payload = device_cert_payload(V2, &state, Uuid::from_u128(1), &signing_key(20), "x");
        let cert = sign(Purpose::Device, V1, &id.identity, payload);
        assert_eq!(
            verify_device_cert(&state, &cert),
            Err(IdentityError::BadEncoding("signature"))
        );
    }
}
