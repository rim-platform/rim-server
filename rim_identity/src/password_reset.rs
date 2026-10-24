use serde::Deserialize;
use uuid::Uuid;

use crate::error::{IdentityError, IdentityResult};
use crate::key_log::KeyState;
use crate::statement::{Purpose, SignedStatement, StatementVersion, check_header, parse, verify};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawPasswordReset {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    principal_id: String,
    challenge_id: Uuid,
    nonce: String,
    audience: String,
}

/// The fields of a password reset: the account's principal, the server's challenge it answers
/// and the server it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordResetProof {
    pub principal_id: String,
    pub challenge_id: Uuid,
    pub nonce: String,
    /// The `host[:port]` or server id the client believes it is resetting at.
    pub audience: String,
}

/// A password reset whose signature has not been checked yet.
///
/// The verifier loads the key log of `principal_id`; only [`UnverifiedPasswordReset::verify`]
/// returns the [`PasswordResetProof`] to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedPasswordReset {
    pub principal_id: String,
    proof: PasswordResetProof,
    statement: SignedStatement,
}

impl UnverifiedPasswordReset {
    /// Parse a version 2 `password-reset` statement without checking its signature.
    ///
    /// # Errors
    /// When the payload is not a `password-reset` statement, or is version 1.
    pub fn read(statement: &SignedStatement) -> IdentityResult<Self> {
        let raw: RawPasswordReset = parse(Purpose::PasswordReset, &statement.payload)?;
        let version = check_header(raw.v, &raw.kind, "password-reset")?;
        if version != StatementVersion::V2 {
            return Err(IdentityError::UnsupportedVersion(raw.v));
        }
        Ok(Self {
            principal_id: raw.principal_id.clone(),
            proof: PasswordResetProof {
                principal_id: raw.principal_id,
                challenge_id: raw.challenge_id,
                nonce: raw.nonce,
                audience: raw.audience,
            },
            statement: statement.clone(),
        })
    }

    /// Check the signature by the recovery key of the key log head `state`, which must be the
    /// statement's principal. A signature by the identity key or a device key does not verify.
    ///
    /// # Errors
    /// `PrincipalMismatch` for another principal's log, `BadSignature` for any other signer.
    pub fn verify(self, state: &KeyState) -> IdentityResult<PasswordResetProof> {
        if state.principal_id != self.principal_id {
            return Err(IdentityError::PrincipalMismatch);
        }
        verify(
            Purpose::PasswordReset,
            StatementVersion::V2,
            &state.recovery_key,
            &self.statement,
        )?;
        Ok(self.proof)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statement::sign;
    use crate::testing::{TestIdentity, password_reset_payload, signing_key};

    fn state(id: &TestIdentity) -> KeyState {
        KeyState::from_inception(&id.inception()).expect("inception")
    }

    #[test]
    fn a_reset_verifies_with_the_recovery_key_only() {
        let id = TestIdentity::with_version(1, StatementVersion::V2);
        let state = state(&id);
        let challenge = Uuid::from_u128(9);
        let stmt = id.password_reset(&state, challenge, "n0nce", "localhost:8080");
        let proof = UnverifiedPasswordReset::read(&stmt)
            .expect("read")
            .verify(&state)
            .expect("verify");
        assert_eq!(proof.principal_id, state.principal_id);
        assert_eq!(proof.challenge_id, challenge);
        assert_eq!(proof.nonce, "n0nce");
        assert_eq!(proof.audience, "localhost:8080");

        let payload = password_reset_payload(&state, challenge, "n0nce", "localhost:8080");
        for signer in [&id.identity, &signing_key(70)] {
            let stmt = sign(
                Purpose::PasswordReset,
                StatementVersion::V2,
                signer,
                &payload,
            );
            assert_eq!(
                UnverifiedPasswordReset::read(&stmt)
                    .expect("read")
                    .verify(&state),
                Err(IdentityError::BadSignature)
            );
        }
        let as_login = sign(Purpose::Login, StatementVersion::V2, &id.recovery, &payload);
        assert_eq!(
            UnverifiedPasswordReset::read(&as_login)
                .expect("read")
                .verify(&state),
            Err(IdentityError::BadSignature)
        );
    }

    #[test]
    fn a_reset_for_another_principal_or_version_is_refused() {
        let id = TestIdentity::with_version(1, StatementVersion::V2);
        let other = TestIdentity::with_version(2, StatementVersion::V2);
        let stmt = id.password_reset(&state(&id), Uuid::from_u128(1), "n", "a");
        assert_eq!(
            UnverifiedPasswordReset::read(&stmt)
                .expect("read")
                .verify(&state(&other)),
            Err(IdentityError::PrincipalMismatch)
        );
        let v1 = sign(
            Purpose::PasswordReset,
            StatementVersion::V1,
            &id.recovery,
            r#"{"v":1,"type":"password-reset","principalId":"p","challengeId":"00000000-0000-0000-0000-000000000001","nonce":"n","audience":"a"}"#,
        );
        assert_eq!(
            UnverifiedPasswordReset::read(&v1),
            Err(IdentityError::UnsupportedVersion(1))
        );
        let login = sign(
            Purpose::PasswordReset,
            StatementVersion::V2,
            &id.recovery,
            r#"{"v":2,"type":"login","principalId":"p","challengeId":"00000000-0000-0000-0000-000000000001","nonce":"n","audience":"a"}"#,
        );
        assert_eq!(
            UnverifiedPasswordReset::read(&login),
            Err(IdentityError::WrongType {
                expected: "password-reset"
            })
        );
    }
}
