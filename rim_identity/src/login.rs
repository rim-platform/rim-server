use serde::Deserialize;
use uuid::Uuid;

use crate::encoding::PublicKey;
use crate::error::IdentityResult;
use crate::statement::{Purpose, SignedStatement, StatementVersion, check_header, parse, verify};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawLoginProof {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    challenge_id: Uuid,
    nonce: String,
    audience: String,
    principal_id: String,
    device_id: Uuid,
}

/// The fields of a login proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginProof {
    pub challenge_id: Uuid,
    pub nonce: String,
    /// The `host[:port]` the client believes it is signing in to. The server compares it with
    /// its own; this is what stops one server from replaying a proof to another.
    pub audience: String,
    pub principal_id: String,
    pub device_id: Uuid,
}

/// A login proof whose signature has not been checked yet.
///
/// The verifier looks up the device key by `principal_id` and `device_id`; only
/// [`UnverifiedLogin::verify`] returns the [`LoginProof`] to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedLogin {
    pub principal_id: String,
    pub device_id: Uuid,
    proof: LoginProof,
    version: StatementVersion,
    statement: SignedStatement,
}

impl UnverifiedLogin {
    /// Parse a login proof without checking its signature.
    pub fn read(statement: &SignedStatement) -> IdentityResult<Self> {
        let raw: RawLoginProof = parse(Purpose::Login, &statement.payload)?;
        let version = check_header(raw.v, &raw.kind, "login")?;
        Ok(Self {
            principal_id: raw.principal_id.clone(),
            device_id: raw.device_id,
            proof: LoginProof {
                challenge_id: raw.challenge_id,
                nonce: raw.nonce,
                audience: raw.audience,
                principal_id: raw.principal_id,
                device_id: raw.device_id,
            },
            version,
            statement: statement.clone(),
        })
    }

    /// Check the signature by the device key.
    pub fn verify(self, device_key: &PublicKey) -> IdentityResult<LoginProof> {
        verify(Purpose::Login, self.version, device_key, &self.statement)?;
        Ok(self.proof)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::IdentityError;
    use crate::key_log::KeyState;
    use crate::statement::StatementVersion::{V1, V2};
    use crate::statement::sign;
    use crate::testing::{TestIdentity, login_payload, signing_key};

    #[test]
    fn login_proof_verifies_with_device_key_only() {
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let device = signing_key(20);
        let challenge = Uuid::from_u128(9);
        let payload = login_payload(
            V1,
            &state,
            Uuid::from_u128(1),
            challenge,
            "n0nce",
            "localhost:8080",
        );
        let stmt = sign(Purpose::Login, V1, &device, payload);

        let unverified = UnverifiedLogin::read(&stmt).expect("read");
        assert_eq!(unverified.principal_id, state.principal_id);
        assert_eq!(unverified.device_id, Uuid::from_u128(1));
        assert_eq!(
            unverified.clone().verify(&PublicKey::from(&id.identity)),
            Err(IdentityError::BadSignature)
        );
        let proof = unverified
            .verify(&PublicKey::from(&device))
            .expect("verify");
        assert_eq!(proof.challenge_id, challenge);
        assert_eq!(proof.nonce, "n0nce");
        assert_eq!(proof.audience, "localhost:8080");
    }

    #[test]
    fn login_proof_rejects_other_statement_types() {
        let stmt = sign(
            Purpose::Login,
            V1,
            &signing_key(20),
            r#"{"v":1,"type":"device","challengeId":"00000000-0000-0000-0000-000000000009","nonce":"n","audience":"a","principalId":"p","deviceId":"00000000-0000-0000-0000-000000000001"}"#,
        );
        assert_eq!(
            UnverifiedLogin::read(&stmt),
            Err(IdentityError::WrongType { expected: "login" })
        );
    }

    #[test]
    fn version_2_login_proof_verifies_with_tagged_signature() {
        let id = TestIdentity::with_version(1, V2);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let device = signing_key(20);
        let stmt = crate::testing::device_login(
            &device,
            &state,
            Uuid::from_u128(1),
            Uuid::from_u128(9),
            "n",
            "localhost:8080",
        );
        assert!(stmt.payload.starts_with(r#"{"v":2,"#));
        assert!(stmt.signature.starts_with("ed25519:"));
        UnverifiedLogin::read(&stmt)
            .expect("read")
            .verify(&PublicKey::from(&device))
            .expect("verify");
    }
}
