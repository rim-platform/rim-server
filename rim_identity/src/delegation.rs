use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::encoding::{PublicKey, b64_fixed};
use crate::error::IdentityError;
use crate::statement::{Purpose, SignedStatement, StatementVersion, check_header, parse, verify};

/// Longest a delegation may run, from `issuedAt` to `expiresAt`.
pub const MAX_DELEGATION_LIFETIME: Duration = Duration::hours(24);

/// The only scope a delegation names in this version.
pub const DELEGATION_SCOPE: &str = "workspace";

/// Bytes in a delegation's `nonce`.
pub const DELEGATION_NONCE_LEN: usize = 16;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawDelegation {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    principal_id: String,
    device_id: Uuid,
    relay: String,
    relay_key_hash: String,
    host: String,
    workspace_id: Uuid,
    scope: String,
    nonce: String,
    issued_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

/// A device's grant to a relay: it may receive one workspace's events from a host for the
/// device's person and forward that device's signed requests to the host until `expires_at`.
/// It never lets the relay act as the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegation {
    pub principal_id: String,
    pub device_id: Uuid,
    /// The relay's server id.
    pub relay: String,
    /// Unpadded base64url SHA-256 of the relay key the device saw.
    pub relay_key_hash: String,
    /// The host's server id.
    pub host: String,
    pub workspace_id: Uuid,
    pub nonce: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Why a delegation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DelegationError {
    #[error(transparent)]
    Statement(#[from] IdentityError),
    #[error("the delegation names another host")]
    WrongHost,
    #[error("the delegation names another relay")]
    WrongRelay,
    #[error("relayKeyHash names no key of the relay that a recovery left standing")]
    RelayKey,
    #[error("the delegation has expired")]
    Expired,
}

impl DelegationError {
    /// A stable reason for a refused subscription.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Statement(IdentityError::BadSignature) => "bad_signature",
            Self::Statement(_) => "invalid_delegation",
            Self::WrongHost => "wrong_host",
            Self::WrongRelay => "wrong_relay",
            Self::RelayKey => "relay_key",
            Self::Expired => "expired",
        }
    }
}

/// Who a delegation must name to be accepted: the host checking it, the relay presenting it,
/// and the hashes of that relay's keys no recovery rotation has replaced.
#[derive(Debug, Clone, Copy)]
pub struct DelegationParties<'a> {
    pub host: &'a str,
    pub relay: &'a str,
    pub relay_key_hashes: &'a [String],
}

/// A delegation whose signature has not been checked yet. The verifier looks the device key
/// up by `principal_id` and `device_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedDelegation {
    delegation: Delegation,
    statement: SignedStatement,
}

impl UnverifiedDelegation {
    /// Parse a delegation without checking its signature: version 2 only, scope `workspace`,
    /// a 16-byte nonce, and at most 24 hours from `issuedAt` to a later `expiresAt`.
    pub fn read(statement: &SignedStatement) -> Result<Self, IdentityError> {
        let raw: RawDelegation = parse(Purpose::Delegation, &statement.payload)?;
        let version = check_header(raw.v, &raw.kind, "delegation")?;
        if version != StatementVersion::V2 {
            return Err(IdentityError::UnsupportedVersion(raw.v));
        }
        if raw.scope != DELEGATION_SCOPE {
            return Err(IdentityError::Invalid(format!(
                "scope must be {DELEGATION_SCOPE}"
            )));
        }
        b64_fixed::<DELEGATION_NONCE_LEN>(&raw.nonce, "nonce")?;
        if raw.expires_at <= raw.issued_at
            || raw.expires_at - raw.issued_at > MAX_DELEGATION_LIFETIME
        {
            return Err(IdentityError::Invalid(
                "a delegation runs for at most 24 hours after it is issued".to_string(),
            ));
        }
        Ok(Self {
            delegation: Delegation {
                principal_id: raw.principal_id,
                device_id: raw.device_id,
                relay: raw.relay,
                relay_key_hash: raw.relay_key_hash,
                host: raw.host,
                workspace_id: raw.workspace_id,
                nonce: raw.nonce,
                issued_at: raw.issued_at,
                expires_at: raw.expires_at,
            },
            statement: statement.clone(),
        })
    }

    /// The fields as read, before the signature is checked.
    pub fn fields(&self) -> &Delegation {
        &self.delegation
    }

    /// Check that the delegation names `parties` and is still ahead of `now`, then its
    /// signature by the device key.
    pub fn verify(
        self,
        device_key: &PublicKey,
        parties: &DelegationParties<'_>,
        now: DateTime<Utc>,
    ) -> Result<Delegation, DelegationError> {
        let d = &self.delegation;
        if d.host != parties.host {
            return Err(DelegationError::WrongHost);
        }
        if d.relay != parties.relay {
            return Err(DelegationError::WrongRelay);
        }
        if !parties.relay_key_hashes.contains(&d.relay_key_hash) {
            return Err(DelegationError::RelayKey);
        }
        if d.expires_at <= now {
            return Err(DelegationError::Expired);
        }
        verify(
            Purpose::Delegation,
            StatementVersion::V2,
            device_key,
            &self.statement,
        )?;
        Ok(self.delegation)
    }
}

/// A JSON string literal.
fn quoted(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

/// The payload text of a delegation, in the field order clients write.
pub fn delegation_payload(d: &Delegation) -> String {
    let time = |t: DateTime<Utc>| quoted(&t.format("%Y-%m-%dT%H:%M:%SZ").to_string());
    format!(
        r#"{{"v":2,"type":"delegation","principalId":{},"deviceId":"{}","relay":{},"relayKeyHash":{},"host":{},"workspaceId":"{}","scope":"{DELEGATION_SCOPE}","nonce":{},"issuedAt":{},"expiresAt":{}}}"#,
        quoted(&d.principal_id),
        d.device_id,
        quoted(&d.relay),
        quoted(&d.relay_key_hash),
        quoted(&d.host),
        d.workspace_id,
        quoted(&d.nonce),
        time(d.issued_at),
        time(d.expires_at),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statement::{sign, sign_with_context};
    use crate::testing::signing_key;

    const PRINCIPAL: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";
    const RELAY: &str = "rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c";
    const RELAY_KEY_HASH: &str = "8UGftl4tBoxZjUo1UP_-zwUlobQJPwj5kZ0NC1oaAaM";
    const HOST: &str = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";
    const A3_PAYLOAD: &str = r#"{"v":2,"type":"delegation","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","relay":"rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c","relayKeyHash":"8UGftl4tBoxZjUo1UP_-zwUlobQJPwj5kZ0NC1oaAaM","host":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","workspaceId":"0199d1a0-0000-7000-8000-000000000002","scope":"workspace","nonce":"cHFyc3R1dnd4eXp7fH1-fw","issuedAt":"2026-10-09T00:00:00Z","expiresAt":"2026-10-10T00:00:00Z"}"#;
    const A3_SIGNATURE: &str = "ed25519:aT8j0zeSdSqWySreIIeRokrjVoYuO-3z1k4IcsFN898M2x_in0B9UlaL5hCUAdw-gCisynSQi3b51QQiZdHlCQ";

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().expect("time")
    }

    fn fields() -> Delegation {
        Delegation {
            principal_id: PRINCIPAL.to_string(),
            device_id: Uuid::from_u128(1),
            relay: RELAY.to_string(),
            relay_key_hash: RELAY_KEY_HASH.to_string(),
            host: HOST.to_string(),
            workspace_id: "0199d1a0-0000-7000-8000-000000000002"
                .parse()
                .expect("uuid"),
            nonce: "cHFyc3R1dnd4eXp7fH1-fw".to_string(),
            issued_at: at("2026-10-09T00:00:00Z"),
            expires_at: at("2026-10-10T00:00:00Z"),
        }
    }

    fn signed(payload: String) -> SignedStatement {
        sign_with_context(Purpose::Delegation.context(), &signing_key(20), payload)
    }

    fn hashes() -> Vec<String> {
        vec![RELAY_KEY_HASH.to_string()]
    }

    fn parties(hashes: &[String]) -> DelegationParties<'_> {
        DelegationParties {
            host: HOST,
            relay: RELAY,
            relay_key_hashes: hashes,
        }
    }

    fn device() -> PublicKey {
        PublicKey::from(&signing_key(20))
    }

    /// The known-answer delegation: its exact payload and signature.
    #[test]
    fn a3_delegation_vector() {
        assert_eq!(delegation_payload(&fields()), A3_PAYLOAD);
        let statement = signed(A3_PAYLOAD.to_string());
        assert_eq!(statement.signature, A3_SIGNATURE);
        let hashes = hashes();
        let read = UnverifiedDelegation::read(&statement).expect("read");
        assert_eq!(read.fields().principal_id, PRINCIPAL);
        let verified = read
            .verify(&device(), &parties(&hashes), at("2026-10-09T12:00:00Z"))
            .expect("verify");
        assert_eq!(verified, fields());
    }

    #[test]
    fn refuses_version_1_unknown_fields_other_scopes_and_bad_nonces() {
        let v1 = sign(
            Purpose::Delegation,
            StatementVersion::V1,
            &signing_key(20),
            A3_PAYLOAD.replace(r#""v":2"#, r#""v":1"#),
        );
        assert_eq!(
            UnverifiedDelegation::read(&v1),
            Err(IdentityError::UnsupportedVersion(1))
        );
        let extra = signed(A3_PAYLOAD.replace(r#""v":2,"#, r#""v":2,"extra":1,"#));
        assert!(matches!(
            UnverifiedDelegation::read(&extra),
            Err(IdentityError::Malformed(_))
        ));
        let host_scope = signed(A3_PAYLOAD.replace(r#""scope":"workspace""#, r#""scope":"host""#));
        assert!(matches!(
            UnverifiedDelegation::read(&host_scope),
            Err(IdentityError::Invalid(_))
        ));
        let short_nonce = signed(A3_PAYLOAD.replace("cHFyc3R1dnd4eXp7fH1-fw", "cHFy"));
        assert!(UnverifiedDelegation::read(&short_nonce).is_err());
        let login = signed(A3_PAYLOAD.replace(r#""type":"delegation""#, r#""type":"login""#));
        assert_eq!(
            UnverifiedDelegation::read(&login),
            Err(IdentityError::WrongType {
                expected: "delegation"
            })
        );
    }

    #[test]
    fn refuses_more_than_24_hours_and_an_end_before_the_start() {
        let mut long = fields();
        long.expires_at = at("2026-10-10T00:00:01Z");
        assert!(matches!(
            UnverifiedDelegation::read(&signed(delegation_payload(&long))),
            Err(IdentityError::Invalid(_))
        ));
        let mut backwards = fields();
        backwards.expires_at = at("2026-10-08T00:00:00Z");
        assert!(UnverifiedDelegation::read(&signed(delegation_payload(&backwards))).is_err());
        let mut short = fields();
        short.expires_at = at("2026-10-09T01:00:00Z");
        assert!(UnverifiedDelegation::read(&signed(delegation_payload(&short))).is_ok());
    }

    #[test]
    fn refuses_another_host_relay_or_relay_key_and_an_expired_one() {
        let statement = signed(A3_PAYLOAD.to_string());
        let read = || UnverifiedDelegation::read(&statement).expect("read");
        let now = at("2026-10-09T12:00:00Z");
        let hashes = hashes();
        let other_host = DelegationParties {
            host: RELAY,
            ..parties(&hashes)
        };
        assert_eq!(
            read().verify(&device(), &other_host, now),
            Err(DelegationError::WrongHost)
        );
        let other_relay = DelegationParties {
            relay: HOST,
            ..parties(&hashes)
        };
        assert_eq!(
            read().verify(&device(), &other_relay, now),
            Err(DelegationError::WrongRelay)
        );
        // After a recovery rotation the relay's earlier keys are no longer listed.
        let recovered = vec!["fvKh1r_va0lqRvlpGojY2V1JzJMQeSESlNpO4OXw6Ak".to_string()];
        assert_eq!(
            read().verify(&device(), &parties(&recovered), now),
            Err(DelegationError::RelayKey)
        );
        assert_eq!(
            read().verify(&device(), &parties(&hashes), at("2026-10-10T00:00:00Z")),
            Err(DelegationError::Expired)
        );
        let wrong_key = read().verify(&PublicKey::from(&signing_key(21)), &parties(&hashes), now);
        assert_eq!(
            wrong_key,
            Err(DelegationError::Statement(IdentityError::BadSignature))
        );
        assert_eq!(wrong_key.unwrap_err().reason(), "bad_signature");
    }

    #[test]
    fn a_signature_for_another_purpose_does_not_verify() {
        let as_request = sign_with_context(
            Purpose::DeviceRequest.context(),
            &signing_key(20),
            A3_PAYLOAD,
        );
        let hashes = hashes();
        assert_eq!(
            UnverifiedDelegation::read(&as_request)
                .expect("read")
                .verify(&device(), &parties(&hashes), at("2026-10-09T12:00:00Z")),
            Err(DelegationError::Statement(IdentityError::BadSignature))
        );
    }
}
