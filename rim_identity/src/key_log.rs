use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::encoding::{PublicKey, b64, principal_id_for_inception, sha256};
use crate::error::{IdentityError, IdentityResult};
use crate::statement::{Purpose, SignedStatement, StatementVersion, check_header, parse, verify};

/// Who signed a rotation entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RotationSigner {
    /// The current identity key: a routine rotation. The recovery key cannot change.
    Identity,
    /// The recovery key derived from the recovery phrase: the identity key was lost or stolen.
    /// The recovery key may change too, and servers revoke every device of the principal.
    Recovery,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawInception {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    seq: u32,
    identity_key: String,
    recovery_key: String,
    #[allow(dead_code)]
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawRotation {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    principal_id: String,
    seq: u32,
    prev: String,
    identity_key: String,
    recovery_key: String,
    signer: RotationSigner,
    #[allow(dead_code)]
    created_at: DateTime<Utc>,
}

/// Only the `principalId` field of a rotation entry, for looking up whose log it extends.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntryPrincipal {
    principal_id: String,
}

/// The state of a key log after verifying it from inception to its head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyState {
    pub principal_id: String,
    /// Seq of the head entry. Device certificates and profile cards name it as `keySeq`.
    pub seq: u32,
    pub identity_key: PublicKey,
    pub recovery_key: PublicKey,
    /// Statement version of the head entry. A log never goes back to an older version: once a
    /// version 2 entry is in, every later entry is version 2.
    pub version: StatementVersion,
    /// SHA-256 of the head entry's payload; the next entry's `prev`.
    head_hash: [u8; 32],
}

impl KeyState {
    /// The `prev` value the next rotation must carry: unpadded base64url SHA-256 of the head
    /// entry's payload.
    pub fn next_prev(&self) -> String {
        b64(&self.head_hash)
    }

    /// Verify an inception entry (seq 0, self-signed by its identity key) and start a log.
    pub fn from_inception(entry: &SignedStatement) -> IdentityResult<Self> {
        let raw: RawInception = parse(Purpose::KeyLog, &entry.payload)?;
        let version = check_header(raw.v, &raw.kind, "inception")?;
        if raw.seq != 0 {
            return Err(IdentityError::SequenceMismatch {
                expected: 0,
                got: raw.seq,
            });
        }
        let identity_key = PublicKey::parse_for(version, &raw.identity_key, "identityKey")?;
        let recovery_key = PublicKey::parse_for(version, &raw.recovery_key, "recoveryKey")?;
        if identity_key == recovery_key {
            return Err(IdentityError::Invalid(
                "identityKey and recoveryKey must differ".to_string(),
            ));
        }
        verify(Purpose::KeyLog, version, &identity_key, entry)?;
        Ok(Self {
            principal_id: principal_id_for_inception(&entry.payload),
            seq: 0,
            identity_key,
            recovery_key,
            version,
            head_hash: sha256(entry.payload.as_bytes()),
        })
    }

    /// Verify `entry` as the next rotation of this log and return the new state.
    pub fn apply(&self, entry: &SignedStatement) -> IdentityResult<(Self, RotationSigner)> {
        let raw: RawRotation = parse(Purpose::KeyLog, &entry.payload)?;
        let version = check_header(raw.v, &raw.kind, "rotate")?;
        if version < self.version {
            return Err(IdentityError::VersionDowngrade {
                head: self.version.number(),
                got: version.number(),
            });
        }
        if raw.principal_id != self.principal_id {
            return Err(IdentityError::PrincipalMismatch);
        }
        let expected = self.seq + 1;
        if raw.seq != expected {
            return Err(IdentityError::SequenceMismatch {
                expected,
                got: raw.seq,
            });
        }
        if raw.prev != b64(&self.head_hash) {
            return Err(IdentityError::ChainMismatch);
        }
        let identity_key = PublicKey::parse_for(version, &raw.identity_key, "identityKey")?;
        let recovery_key = PublicKey::parse_for(version, &raw.recovery_key, "recoveryKey")?;
        if identity_key == self.identity_key {
            return Err(IdentityError::Invalid(
                "a rotation must change the identity key".to_string(),
            ));
        }
        if identity_key == recovery_key {
            return Err(IdentityError::Invalid(
                "identityKey and recoveryKey must differ".to_string(),
            ));
        }
        match raw.signer {
            RotationSigner::Identity => {
                if recovery_key != self.recovery_key {
                    return Err(IdentityError::Invalid(
                        "only the recovery key can change the recovery key".to_string(),
                    ));
                }
                verify(Purpose::KeyLog, version, &self.identity_key, entry)?;
            }
            RotationSigner::Recovery => {
                verify(Purpose::KeyLog, version, &self.recovery_key, entry)?
            }
        }
        let next = Self {
            principal_id: self.principal_id.clone(),
            seq: raw.seq,
            identity_key,
            recovery_key,
            version,
            head_hash: sha256(entry.payload.as_bytes()),
        };
        Ok((next, raw.signer))
    }

    /// Verify a whole log, oldest entry first.
    pub fn replay(entries: &[SignedStatement]) -> IdentityResult<Self> {
        let (first, rest) = entries
            .split_first()
            .ok_or_else(|| IdentityError::Invalid("key log is empty".to_string()))?;
        let mut state = Self::from_inception(first)?;
        for entry in rest {
            state = state.apply(entry)?.0;
        }
        Ok(state)
    }
}

/// The principal a rotation entry claims to extend. Unverified: use it only to find the log,
/// then [`KeyState::apply`] checks it against that log.
pub fn rotation_principal_id(entry: &SignedStatement) -> IdentityResult<String> {
    let raw: EntryPrincipal = parse(Purpose::KeyLog, &entry.payload)?;
    Ok(raw.principal_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::is_principal_id;
    use crate::statement::StatementVersion::{V1, V2};
    use crate::statement::sign;
    use crate::testing::{
        TestIdentity, inception_payload, key_text, rotation_payload, signing_key,
    };

    #[test]
    fn inception_fixes_principal_id_from_payload() {
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        assert!(is_principal_id(&state.principal_id));
        assert_eq!(state.principal_id, id.principal_id());
        assert_eq!(state.seq, 0);
        assert_eq!(state.identity_key, PublicKey::from(&id.identity));
    }

    #[test]
    fn inception_must_be_self_signed() {
        let id = TestIdentity::new(1);
        let payload = inception_payload(V1, &id.identity, &id.recovery);
        let entry = sign(Purpose::KeyLog, V1, &id.recovery, payload);
        assert_eq!(
            KeyState::from_inception(&entry),
            Err(IdentityError::BadSignature)
        );
    }

    #[test]
    fn inception_rejects_same_identity_and_recovery_key() {
        let key = signing_key(1);
        let entry = sign(Purpose::KeyLog, V1, &key, inception_payload(V1, &key, &key));
        assert!(matches!(
            KeyState::from_inception(&entry),
            Err(IdentityError::Invalid(_))
        ));
    }

    #[test]
    fn inception_rejects_unknown_fields_and_wrong_seq() {
        let id = TestIdentity::new(1);
        let extra =
            inception_payload(V1, &id.identity, &id.recovery).replacen('{', r#"{"x":1,"#, 1);
        let entry = sign(Purpose::KeyLog, V1, &id.identity, extra);
        assert!(matches!(
            KeyState::from_inception(&entry),
            Err(IdentityError::Malformed(_))
        ));

        let wrong_seq =
            inception_payload(V1, &id.identity, &id.recovery).replace(r#""seq":0"#, r#""seq":1"#);
        let entry = sign(Purpose::KeyLog, V1, &id.identity, wrong_seq);
        assert_eq!(
            KeyState::from_inception(&entry),
            Err(IdentityError::SequenceMismatch {
                expected: 0,
                got: 1
            })
        );
    }

    #[test]
    fn duplicate_fields_are_rejected() {
        let id = TestIdentity::new(1);
        let dup = inception_payload(V1, &id.identity, &id.recovery).replacen(
            '{',
            &format!(r#"{{"identityKey":"{}","#, key_text(V1, &signing_key(9))),
            1,
        );
        let entry = sign(Purpose::KeyLog, V1, &id.identity, dup);
        assert!(matches!(
            KeyState::from_inception(&entry),
            Err(IdentityError::Malformed(_))
        ));
    }

    #[test]
    fn identity_rotation_keeps_recovery_key() {
        let mut id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let rotation = id.rotate_with_identity(&state, 2);
        let (next, signer) = state.apply(&rotation).expect("rotate");
        assert_eq!(signer, RotationSigner::Identity);
        assert_eq!(next.seq, 1);
        assert_eq!(next.principal_id, state.principal_id);
        assert_eq!(next.identity_key, PublicKey::from(&signing_key(2)));
        assert_eq!(next.recovery_key, state.recovery_key);
    }

    #[test]
    fn identity_rotation_cannot_change_recovery_key() {
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let payload = rotation_payload(
            V1,
            &state,
            &signing_key(2),
            &signing_key(3),
            RotationSigner::Identity,
        );
        let entry = sign(Purpose::KeyLog, V1, &id.identity, payload);
        assert!(matches!(
            state.apply(&entry),
            Err(IdentityError::Invalid(_))
        ));
    }

    #[test]
    fn recovery_rotation_may_change_recovery_key() {
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let payload = rotation_payload(
            V1,
            &state,
            &signing_key(2),
            &signing_key(3),
            RotationSigner::Recovery,
        );
        let entry = sign(Purpose::KeyLog, V1, &id.recovery, payload);
        let (next, signer) = state.apply(&entry).expect("recover");
        assert_eq!(signer, RotationSigner::Recovery);
        assert_eq!(next.recovery_key, PublicKey::from(&signing_key(3)));
    }

    #[test]
    fn rotation_signed_by_wrong_key_fails() {
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let payload = rotation_payload(
            V1,
            &state,
            &signing_key(2),
            &id.recovery,
            RotationSigner::Recovery,
        );
        // Claims the recovery key but is signed by the identity key.
        let entry = sign(Purpose::KeyLog, V1, &id.identity, payload);
        assert_eq!(state.apply(&entry), Err(IdentityError::BadSignature));
    }

    #[test]
    fn rotation_must_chain_and_follow_seq() {
        let mut id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let first = id.rotate_with_identity(&state, 2);
        let (next, _) = state.apply(&first).expect("first");

        // Replaying the first rotation against the new head fails on seq.
        assert!(matches!(
            next.apply(&first),
            Err(IdentityError::SequenceMismatch { .. })
        ));

        // A rotation with the right seq but the wrong prev fails on the chain.
        let payload = rotation_payload(
            V1,
            &next,
            &signing_key(4),
            &id.recovery,
            RotationSigner::Identity,
        )
        .replace(&b64(&next.head_hash), &b64(&[0; 32]));
        let entry = sign(Purpose::KeyLog, V1, &signing_key(2), payload);
        assert_eq!(next.apply(&entry), Err(IdentityError::ChainMismatch));
    }

    #[test]
    fn rotation_for_another_principal_fails() {
        let id = TestIdentity::new(1);
        let other = TestIdentity::new(5);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let other_state = KeyState::from_inception(&other.inception()).expect("other");
        let payload = rotation_payload(
            V1,
            &other_state,
            &signing_key(2),
            &other.recovery,
            RotationSigner::Identity,
        );
        let entry = sign(Purpose::KeyLog, V1, &other.identity, payload);
        assert_eq!(state.apply(&entry), Err(IdentityError::PrincipalMismatch));
    }

    #[test]
    fn rotation_must_change_identity_key() {
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let payload = rotation_payload(
            V1,
            &state,
            &id.identity,
            &id.recovery,
            RotationSigner::Identity,
        );
        let entry = sign(Purpose::KeyLog, V1, &id.identity, payload);
        assert!(matches!(
            state.apply(&entry),
            Err(IdentityError::Invalid(_))
        ));
    }

    #[test]
    fn replay_verifies_whole_log() {
        let mut id = TestIdentity::new(1);
        let inception = id.inception();
        let state = KeyState::from_inception(&inception).expect("inception");
        let r1 = id.rotate_with_identity(&state, 2);
        let (state1, _) = state.apply(&r1).expect("r1");
        let r2 = id.rotate_with_identity(&state1, 3);
        let replayed = KeyState::replay(&[inception.clone(), r1, r2.clone()]).expect("replay");
        assert_eq!(replayed.seq, 2);
        assert_eq!(replayed.identity_key, PublicKey::from(&signing_key(3)));

        // Skipping an entry fails.
        assert!(matches!(
            KeyState::replay(&[inception, r2]),
            Err(IdentityError::SequenceMismatch { .. })
        ));
        assert!(KeyState::replay(&[]).is_err());
    }

    #[test]
    fn rotation_principal_can_be_read_before_verifying() {
        let mut id = TestIdentity::new(1);
        let inception = id.inception();
        let state = KeyState::from_inception(&inception).expect("inception");
        let rotation = id.rotate_with_identity(&state, 2);
        assert_eq!(rotation_principal_id(&rotation), Ok(state.principal_id));
    }

    #[test]
    fn key_log_signature_is_not_a_device_signature() {
        let id = TestIdentity::new(1);
        let payload = inception_payload(V1, &id.identity, &id.recovery);
        let entry = sign(Purpose::Device, V1, &id.identity, payload);
        assert_eq!(
            KeyState::from_inception(&entry),
            Err(IdentityError::BadSignature)
        );
    }

    #[test]
    fn version_2_inception_uses_tagged_keys_and_signature() {
        let id = TestIdentity::with_version(1, V2);
        let inception = id.inception();
        assert!(inception.signature.starts_with("ed25519:"));
        assert!(inception.payload.contains(r#""identityKey":"ed25519:"#));
        let state = KeyState::from_inception(&inception).expect("inception");
        assert_eq!(state.version, V2);
        assert_eq!(state.principal_id, id.principal_id());
        // The id hashes the payload bytes, so the same keys under version 1 give another id.
        assert_ne!(state.principal_id, TestIdentity::new(1).principal_id());
    }

    #[test]
    fn version_2_inception_with_bare_key_or_signature_fails() {
        let id = TestIdentity::with_version(1, V2);
        // A version 2 payload spelling keys the version 1 way.
        let payload =
            inception_payload(V1, &id.identity, &id.recovery).replacen(r#""v":1"#, r#""v":2"#, 1);
        let entry = sign(Purpose::KeyLog, V2, &id.identity, payload);
        assert_eq!(
            KeyState::from_inception(&entry),
            Err(IdentityError::BadEncoding("identityKey"))
        );
        // A version 2 payload with a bare signature.
        let entry = sign(
            Purpose::KeyLog,
            V1,
            &id.identity,
            inception_payload(V2, &id.identity, &id.recovery),
        );
        assert_eq!(
            KeyState::from_inception(&entry),
            Err(IdentityError::BadEncoding("signature"))
        );
    }

    #[test]
    fn version_1_log_continues_with_version_2_rotations() {
        let mut id = TestIdentity::new(1);
        let inception = id.inception();
        let state = KeyState::from_inception(&inception).expect("inception");
        assert_eq!(state.version, V1);

        id.version = V2;
        let r1 = id.rotate_with_identity(&state, 2);
        let (state1, _) = state.apply(&r1).expect("v2 rotation of a v1 log");
        assert_eq!(state1.version, V2);
        assert_eq!(state1.principal_id, state.principal_id);
        assert_eq!(state1.identity_key, PublicKey::from(&signing_key(2)));
        // The recovery key named bare in the inception and tagged in the rotation is one key.
        assert_eq!(state1.recovery_key, state.recovery_key);

        let r2 = id.recover(&state1, 3);
        let replayed = KeyState::replay(&[inception, r1, r2]).expect("replay mixed log");
        assert_eq!(replayed.seq, 2);
        assert_eq!(replayed.version, V2);
        assert_eq!(replayed.principal_id, state.principal_id);
    }

    #[test]
    fn log_never_goes_back_to_version_1() {
        let mut id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        id.version = V2;
        let (state1, _) = state
            .apply(&id.rotate_with_identity(&state, 2))
            .expect("v2 rotation");
        id.version = V1;
        let back = id.rotate_with_identity(&state1, 3);
        assert_eq!(
            state1.apply(&back),
            Err(IdentityError::VersionDowngrade { head: 2, got: 1 })
        );

        // A version 2 inception cannot be followed by version 1 either.
        let mut id = TestIdentity::with_version(5, V2);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        id.version = V1;
        assert_eq!(
            state.apply(&id.rotate_with_identity(&state, 6)),
            Err(IdentityError::VersionDowngrade { head: 2, got: 1 })
        );
    }

    #[test]
    fn unsupported_statement_version_is_refused() {
        let id = TestIdentity::new(1);
        let payload =
            inception_payload(V1, &id.identity, &id.recovery).replacen(r#""v":1"#, r#""v":3"#, 1);
        let entry = sign(Purpose::KeyLog, V1, &id.identity, payload);
        assert_eq!(
            KeyState::from_inception(&entry),
            Err(IdentityError::UnsupportedVersion(3))
        );
    }

    #[test]
    fn version_2_rotation_naming_an_unknown_algorithm_is_refused() {
        let mut id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        id.version = V2;
        let payload = rotation_payload(
            V2,
            &state,
            &signing_key(2),
            &id.recovery,
            RotationSigner::Identity,
        )
        .replacen(
            r#""identityKey":"ed25519:"#,
            r#""identityKey":"ed25519-mldsa44:"#,
            1,
        );
        let entry = sign(Purpose::KeyLog, V2, &id.identity, payload);
        assert_eq!(
            state.apply(&entry),
            Err(IdentityError::UnsupportedAlgorithm(
                "identityKey",
                "ed25519-mldsa44".to_string()
            ))
        );
    }
}
