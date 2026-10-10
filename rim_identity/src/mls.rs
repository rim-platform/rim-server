//! The statements behind end-to-end encryption: the `mls-key` statement in which a device key
//! certifies the device's MLS signature key, the MLS credential identity that carries it in a
//! leaf, and the `mls-readd` request a device signs when it lost its group state.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::encoding::{Algorithm, PublicKey, b64_fixed, is_principal_id};
use crate::error::{IdentityError, IdentityResult};
use crate::statement::{Purpose, SignedStatement, StatementVersion, check_header, parse, verify};

/// Largest MLS credential identity accepted, in bytes: a capped `mls-key` payload, JSON-escaped,
/// plus its signature.
pub const MAX_MLS_CREDENTIAL_LEN: usize = 40 * 1024;

/// Length of a re-add request's nonce, in bytes.
pub const MLS_READD_NONCE_LEN: usize = 16;

fn invalid(message: impl Into<String>) -> IdentityError {
    IdentityError::Invalid(message.into())
}

fn check_principal_id(principal_id: &str) -> IdentityResult<()> {
    if is_principal_id(principal_id) {
        Ok(())
    } else {
        Err(invalid("principalId is not a principal id"))
    }
}

/// Both statements here exist only in version 2.
fn check_v2(version: StatementVersion, kind: &str) -> IdentityResult<()> {
    if version == StatementVersion::V2 {
        Ok(())
    } else {
        Err(invalid(format!(
            "an {kind} statement must be a version 2 statement"
        )))
    }
}

/// A field that must be present, `null` or not.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawMlsKey {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    principal_id: String,
    device_id: Uuid,
    mls_key: String,
    issued_at: DateTime<Utc>,
}

/// A device's MLS signature key, certified by its device key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsKeyStatement {
    pub principal_id: String,
    pub device_id: Uuid,
    /// Ed25519, the signature scheme of every cipher suite RIM uses.
    pub mls_key: PublicKey,
    pub issued_at: DateTime<Utc>,
}

/// An `mls-key` statement whose signature has not been checked yet.
///
/// The verifier looks up the device key by `principal_id` and `device_id`; only
/// [`UnverifiedMlsKey::verify`] returns the [`MlsKeyStatement`] to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedMlsKey {
    pub principal_id: String,
    pub device_id: Uuid,
    fields: MlsKeyStatement,
    statement: SignedStatement,
}

impl UnverifiedMlsKey {
    /// Parse an `mls-key` statement and check every field rule, without its signature.
    pub fn read(statement: &SignedStatement) -> IdentityResult<Self> {
        let raw: RawMlsKey = parse(Purpose::MlsKey, &statement.payload)?;
        let version = check_header(raw.v, &raw.kind, "mls-key")?;
        check_v2(version, "mls-key")?;
        check_principal_id(&raw.principal_id)?;
        let mls_key = PublicKey::parse_for(version, &raw.mls_key, "mlsKey")?;
        if mls_key.algorithm() != Algorithm::Ed25519 {
            return Err(invalid("mlsKey must be an Ed25519 key"));
        }
        Ok(Self {
            principal_id: raw.principal_id.clone(),
            device_id: raw.device_id,
            fields: MlsKeyStatement {
                principal_id: raw.principal_id,
                device_id: raw.device_id,
                mls_key,
                issued_at: raw.issued_at,
            },
            statement: statement.clone(),
        })
    }

    /// Check the signature by the device key the statement names.
    pub fn verify(self, device_key: &PublicKey) -> IdentityResult<MlsKeyStatement> {
        verify(
            Purpose::MlsKey,
            StatementVersion::V2,
            device_key,
            &self.statement,
        )?;
        Ok(self.fields)
    }
}

/// Verify an `mls-key` statement signed by `device_key`. The caller checks that its principal
/// and device are the ones `device_key` belongs to.
pub fn verify_mls_key(
    device_key: &PublicKey,
    statement: &SignedStatement,
) -> IdentityResult<MlsKeyStatement> {
    UnverifiedMlsKey::read(statement)?.verify(device_key)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCredential {
    payload: String,
    signature: String,
}

/// The identity bytes of an MLS basic credential: the UTF-8 JSON
/// `{"payload":"<payload>","signature":"<signature>"}` of an `mls-key` statement.
pub fn mls_credential_identity(statement: &SignedStatement) -> Vec<u8> {
    // Two string fields always serialize.
    serde_json::to_vec(&RawCredential {
        payload: statement.payload.clone(),
        signature: statement.signature.clone(),
    })
    .unwrap_or_default()
}

/// Read the `mls-key` statement out of an MLS basic credential's identity bytes. Unknown and
/// duplicate fields are refused, as in a statement.
pub fn read_mls_credential(identity: &[u8]) -> IdentityResult<SignedStatement> {
    if identity.len() > MAX_MLS_CREDENTIAL_LEN {
        return Err(IdentityError::TooLarge {
            max: MAX_MLS_CREDENTIAL_LEN,
        });
    }
    let text = std::str::from_utf8(identity)
        .map_err(|_| IdentityError::Malformed("credential identity is not UTF-8".to_string()))?;
    let raw: RawCredential =
        serde_json::from_str(text).map_err(|e| IdentityError::Malformed(e.to_string()))?;
    Ok(SignedStatement {
        payload: raw.payload,
        signature: raw.signature,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawMlsReadd {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    principal_id: String,
    device_id: Uuid,
    conversation_id: Uuid,
    incarnation: u32,
    #[serde(deserialize_with = "present")]
    last_epoch: Option<u64>,
    nonce: String,
    issued_at: DateTime<Utc>,
}

/// A device's request to be removed and added again in one conversation's group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlsReaddRequest {
    pub principal_id: String,
    pub device_id: Uuid,
    pub conversation_id: Uuid,
    /// At least 1.
    pub incarnation: u32,
    /// The last epoch the device processed; `None` when it has no group state at all.
    pub last_epoch: Option<u64>,
    pub nonce: [u8; MLS_READD_NONCE_LEN],
    pub issued_at: DateTime<Utc>,
}

/// An `mls-readd` request whose signature has not been checked yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedMlsReadd {
    pub principal_id: String,
    pub device_id: Uuid,
    fields: MlsReaddRequest,
    statement: SignedStatement,
}

impl UnverifiedMlsReadd {
    /// Parse a re-add request and check every field rule, without its signature.
    pub fn read(statement: &SignedStatement) -> IdentityResult<Self> {
        let raw: RawMlsReadd = parse(Purpose::MlsReadd, &statement.payload)?;
        let version = check_header(raw.v, &raw.kind, "mls-readd")?;
        check_v2(version, "mls-readd")?;
        check_principal_id(&raw.principal_id)?;
        if raw.incarnation == 0 {
            return Err(invalid("incarnation must be at least 1"));
        }
        let nonce = b64_fixed::<MLS_READD_NONCE_LEN>(&raw.nonce, "nonce")?;
        Ok(Self {
            principal_id: raw.principal_id.clone(),
            device_id: raw.device_id,
            fields: MlsReaddRequest {
                principal_id: raw.principal_id,
                device_id: raw.device_id,
                conversation_id: raw.conversation_id,
                incarnation: raw.incarnation,
                last_epoch: raw.last_epoch,
                nonce,
                issued_at: raw.issued_at,
            },
            statement: statement.clone(),
        })
    }

    /// Check the signature by the requesting device's key.
    pub fn verify(self, device_key: &PublicKey) -> IdentityResult<MlsReaddRequest> {
        verify(
            Purpose::MlsReadd,
            StatementVersion::V2,
            device_key,
            &self.statement,
        )?;
        Ok(self.fields)
    }
}

/// Verify a re-add request signed by `device_key`. The caller checks that its principal and
/// device are the ones `device_key` belongs to, and that it names the conversation it acts on.
pub fn verify_mls_readd(
    device_key: &PublicKey,
    statement: &SignedStatement,
) -> IdentityResult<MlsReaddRequest> {
    UnverifiedMlsReadd::read(statement)?.verify(device_key)
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::statement::StatementVersion::{V1, V2};
    use crate::statement::sign;
    use crate::testing::{
        TestIdentity, device_mls_key, device_mls_readd, mls_key_payload, mls_readd_payload,
        signing_key,
    };

    const PRINCIPAL_ID: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";
    const MLS_KEY: &str = "ed25519:6S62BU_pvGgqG887dZ9lqzikz72BxNHzNC5Myc3tiws";
    const MLS_KEY_PAYLOAD: &str = r#"{"v":2,"type":"mls-key","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","mlsKey":"ed25519:6S62BU_pvGgqG887dZ9lqzikz72BxNHzNC5Myc3tiws","issuedAt":"2026-10-09T00:00:00Z"}"#;
    const MLS_KEY_SIGNATURE: &str = "ed25519:6TN4OHOn3VKI00okNthNJOJkuAg1afEO9h-63L9T6VoQxeYA1acXMqTuLBu5_flkzCjPRssEWpP6LkVCi74EDA";
    const CREDENTIAL_SHA256: &str =
        "b7334437dc6e0da6588fbfc37540416efc0497c0a75d4eccae1fd92cae969a11";
    const READD_PAYLOAD: &str = r#"{"v":2,"type":"mls-readd","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","conversationId":"0199d1a0-0000-7000-8000-000000000001","incarnation":1,"lastEpoch":41,"nonce":"AAECAwQFBgcICQoLDA0ODw","issuedAt":"2026-10-09T00:00:00Z"}"#;
    const READD_SIGNATURE: &str = "ed25519:UNry-YBfbQv1nUxStNVtA9jirfxTM5rLE-JA40KZGS9c-tCyl90KEq4W1OxmoxL0msvG62Gc5msFvpCI_h_lCw";

    fn device() -> PublicKey {
        PublicKey::from(&signing_key(20))
    }

    fn conversation() -> Uuid {
        "0199d1a0-0000-7000-8000-000000000001"
            .parse()
            .expect("uuid")
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn signed_mls_key(payload: String) -> SignedStatement {
        sign(Purpose::MlsKey, V2, &signing_key(20), payload)
    }

    fn signed_readd(payload: String) -> SignedStatement {
        sign(Purpose::MlsReadd, V2, &signing_key(20), payload)
    }

    fn readd_payload(incarnation: u32, last_epoch: Option<u64>) -> String {
        mls_readd_payload(
            PRINCIPAL_ID,
            Uuid::from_u128(1),
            conversation(),
            incarnation,
            last_epoch,
            &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        )
    }

    #[test]
    fn known_answer_mls_key_statement_and_credential() {
        assert_eq!(
            TestIdentity::with_version(1, V2).principal_id(),
            PRINCIPAL_ID
        );
        let statement = device_mls_key(
            &signing_key(20),
            PRINCIPAL_ID,
            Uuid::from_u128(1),
            &signing_key(40),
        );
        assert_eq!(statement.payload, MLS_KEY_PAYLOAD);
        assert_eq!(statement.signature, MLS_KEY_SIGNATURE);

        let verified = verify_mls_key(&device(), &statement).expect("verify");
        assert_eq!(verified.mls_key.to_string(), MLS_KEY);
        assert_eq!(verified.principal_id, PRINCIPAL_ID);
        assert_eq!(verified.device_id, Uuid::from_u128(1));
        assert_eq!(
            verified.mls_key.key_bytes(),
            signing_key(40).verifying_key().to_bytes().to_vec()
        );

        let identity = mls_credential_identity(&statement);
        assert_eq!(identity.len(), 382);
        assert_eq!(hex(&Sha256::digest(&identity)), CREDENTIAL_SHA256);
        assert_eq!(read_mls_credential(&identity), Ok(statement));
    }

    #[test]
    fn mls_key_signed_by_another_key_or_purpose_fails() {
        let payload = mls_key_payload(PRINCIPAL_ID, Uuid::from_u128(1), &signing_key(40));
        let by_mls_key = sign(Purpose::MlsKey, V2, &signing_key(40), payload.clone());
        assert_eq!(
            verify_mls_key(&device(), &by_mls_key),
            Err(IdentityError::BadSignature)
        );
        let as_login = sign(Purpose::Login, V2, &signing_key(20), payload);
        assert_eq!(
            verify_mls_key(&device(), &as_login),
            Err(IdentityError::BadSignature)
        );
    }

    #[test]
    fn mls_key_field_rules() {
        let payload = mls_key_payload(PRINCIPAL_ID, Uuid::from_u128(1), &signing_key(40));
        let v1 = payload.replacen(r#""v":2"#, r#""v":1"#, 1);
        assert_eq!(
            verify_mls_key(&device(), &sign(Purpose::MlsKey, V1, &signing_key(20), v1)),
            Err(invalid(
                "an mls-key statement must be a version 2 statement"
            ))
        );
        let unknown = payload.replacen('{', r#"{"extra":1,"#, 1);
        assert!(matches!(
            verify_mls_key(&device(), &signed_mls_key(unknown)),
            Err(IdentityError::Malformed(_))
        ));
        let duplicate = payload.replacen(
            '{',
            r#"{"deviceId":"00000000-0000-0000-0000-000000000002","#,
            1,
        );
        assert!(matches!(
            verify_mls_key(&device(), &signed_mls_key(duplicate)),
            Err(IdentityError::Malformed(_))
        ));
        let wrong_type = payload.replacen("mls-key", "mls-readd", 1);
        assert_eq!(
            verify_mls_key(&device(), &signed_mls_key(wrong_type)),
            Err(IdentityError::WrongType {
                expected: "mls-key"
            })
        );
        let bad_principal = payload.replacen(PRINCIPAL_ID, "rimp1_short", 1);
        assert_eq!(
            verify_mls_key(&device(), &signed_mls_key(bad_principal)),
            Err(invalid("principalId is not a principal id"))
        );
        let untagged = payload.replacen("ed25519:", "", 1);
        assert_eq!(
            verify_mls_key(&device(), &signed_mls_key(untagged)),
            Err(IdentityError::BadEncoding("mlsKey"))
        );
        let missing = payload.replacen(r#","issuedAt":"2026-10-09T00:00:00Z""#, "", 1);
        assert!(matches!(
            verify_mls_key(&device(), &signed_mls_key(missing)),
            Err(IdentityError::Malformed(_))
        ));
    }

    #[test]
    fn finding_27_a_malformed_payload_keeps_the_parser_text_out_of_the_message() {
        let unknown = READD_PAYLOAD.replacen('{', r#"{"extra":1,"#, 1);
        let err = verify_mls_readd(&device(), &signed_readd(unknown)).unwrap_err();
        assert_eq!(err.to_string(), "statement payload is not valid");
        assert!(
            err.detail().is_some_and(|d| d.contains("unknown field")),
            "{err:?}"
        );
        assert_eq!(IdentityError::BadSignature.detail(), None);
    }

    #[test]
    fn credential_identity_is_strict() {
        let statement = signed_mls_key(mls_key_payload(
            PRINCIPAL_ID,
            Uuid::from_u128(1),
            &signing_key(40),
        ));
        let extra = serde_json::json!({
            "payload": statement.payload,
            "signature": statement.signature,
            "extra": 1,
        })
        .to_string();
        assert!(matches!(
            read_mls_credential(extra.as_bytes()),
            Err(IdentityError::Malformed(_))
        ));
        assert!(matches!(
            read_mls_credential(&[0xff, 0xfe]),
            Err(IdentityError::Malformed(_))
        ));
        assert_eq!(
            read_mls_credential(&vec![b' '; MAX_MLS_CREDENTIAL_LEN + 1]),
            Err(IdentityError::TooLarge {
                max: MAX_MLS_CREDENTIAL_LEN
            })
        );
    }

    #[test]
    fn known_answer_readd_request() {
        let statement = device_mls_readd(
            &signing_key(20),
            PRINCIPAL_ID,
            Uuid::from_u128(1),
            conversation(),
            1,
            Some(41),
            &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        );
        assert_eq!(statement.payload, READD_PAYLOAD);
        assert_eq!(statement.signature, READD_SIGNATURE);
        let request = verify_mls_readd(&device(), &statement).expect("verify");
        assert_eq!(request.principal_id, PRINCIPAL_ID);
        assert_eq!(request.device_id, Uuid::from_u128(1));
        assert_eq!(request.conversation_id, conversation());
        assert_eq!(request.incarnation, 1);
        assert_eq!(request.last_epoch, Some(41));
        assert_eq!(request.nonce[15], 15);
    }

    #[test]
    fn readd_field_rules() {
        let none = verify_mls_readd(&device(), &signed_readd(readd_payload(2, None)))
            .expect("null lastEpoch");
        assert_eq!(none.last_epoch, None);
        assert_eq!(
            verify_mls_readd(&device(), &signed_readd(readd_payload(0, Some(1)))),
            Err(invalid("incarnation must be at least 1"))
        );
        let missing = readd_payload(1, None).replacen(r#""lastEpoch":null,"#, "", 1);
        assert!(matches!(
            verify_mls_readd(&device(), &signed_readd(missing)),
            Err(IdentityError::Malformed(_))
        ));
        let short_nonce = readd_payload(1, None).replacen("AAECAwQFBgcICQoLDA0ODw", "AAEC", 1);
        assert_eq!(
            verify_mls_readd(&device(), &signed_readd(short_nonce)),
            Err(IdentityError::BadEncoding("nonce"))
        );
        let as_mls_key = sign(
            Purpose::MlsKey,
            V2,
            &signing_key(20),
            readd_payload(1, None),
        );
        assert_eq!(
            verify_mls_readd(&device(), &as_mls_key),
            Err(IdentityError::BadSignature)
        );
    }
}
