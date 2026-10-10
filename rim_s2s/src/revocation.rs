//! `revocation-ack`: a host's signed acknowledgement that it applied a device revocation.

use rim_identity::{PublicKey, SignedStatement};
use serde::Deserialize;

use crate::error::StatementResult;
use crate::host::sha256_b64;
use crate::signer::ServerSigner;
use crate::statement::{self, STATEMENT_VERSION, ServerStatement, parse_payload};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawRevocationAck {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    host: String,
    key_seq: u32,
    principal_id: String,
    device_id: String,
    revocation_sha256: String,
    applied_at: String,
}

/// A verified revocation acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationAck {
    pub host: String,
    pub key_seq: u32,
    pub principal_id: String,
    pub device_id: String,
    /// Unpadded base64url SHA-256 of the revocation payload text.
    pub revocation_sha256: String,
    pub applied_at: String,
}

impl RevocationAck {
    /// Whether this acknowledges the revocation with payload text `revocation_payload`.
    pub fn acknowledges(&self, revocation_payload: &str) -> bool {
        self.revocation_sha256 == sha256_b64(revocation_payload.as_bytes())
    }
}

/// The fields of an acknowledgement to sign.
#[derive(Debug, Clone, Copy)]
pub struct RevocationAckFields<'a> {
    pub host: &'a str,
    pub key_seq: u32,
    pub principal_id: &'a str,
    pub device_id: &'a str,
    /// The revocation's payload text, hashed into `revocationSha256`.
    pub revocation_payload: &'a str,
    pub applied_at: &'a str,
}

/// A JSON string literal.
fn quoted(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

/// The payload text of an acknowledgement, in its fixed field order.
pub fn revocation_ack_payload(fields: &RevocationAckFields<'_>) -> String {
    format!(
        r#"{{"v":2,"type":"revocation-ack","host":{},"keySeq":{},"principalId":{},"deviceId":{},"revocationSha256":"{}","appliedAt":{}}}"#,
        quoted(fields.host),
        fields.key_seq,
        quoted(fields.principal_id),
        quoted(fields.device_id),
        sha256_b64(fields.revocation_payload.as_bytes()),
        quoted(fields.applied_at),
    )
}

/// Sign an acknowledgement.
pub fn sign_revocation_ack(
    signer: &ServerSigner,
    fields: &RevocationAckFields<'_>,
) -> SignedStatement {
    statement::sign(
        signer,
        ServerStatement::RevocationAck,
        revocation_ack_payload(fields),
    )
}

/// Read an acknowledgement's fields without checking its signature, to find the key it names.
pub fn read_revocation_ack(statement: &SignedStatement) -> StatementResult<RevocationAck> {
    let raw: RawRevocationAck = parse_payload(ServerStatement::RevocationAck, &statement.payload)?;
    if raw.v != STATEMENT_VERSION {
        return Err(rim_identity::IdentityError::UnsupportedVersion(raw.v).into());
    }
    if raw.kind != "revocation-ack" {
        return Err(rim_identity::IdentityError::WrongType {
            expected: "revocation-ack",
        }
        .into());
    }
    Ok(RevocationAck {
        host: raw.host,
        key_seq: raw.key_seq,
        principal_id: raw.principal_id,
        device_id: raw.device_id,
        revocation_sha256: raw.revocation_sha256,
        applied_at: raw.applied_at,
    })
}

/// Check an acknowledgement's signature by `key` (the host's key at its `keySeq`) and read it.
pub fn verify_revocation_ack(
    key: &PublicKey,
    statement: &SignedStatement,
) -> StatementResult<RevocationAck> {
    let ack = read_revocation_ack(statement)?;
    statement::verify(ServerStatement::RevocationAck, key, statement)?;
    Ok(ack)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST_ID: &str = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";
    const PRINCIPAL: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";
    const DEVICE: &str = "00000000-0000-0000-0000-000000000001";
    const REVOCATION: &str = r#"{"v":2,"type":"device-revoke","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","signer":"device","issuedAt":"2026-10-09T00:00:00Z"}"#;

    fn host() -> ServerSigner {
        ServerSigner::from_seed([40; 32])
    }

    fn fields() -> RevocationAckFields<'static> {
        RevocationAckFields {
            host: HOST_ID,
            key_seq: 0,
            principal_id: PRINCIPAL,
            device_id: DEVICE,
            revocation_payload: REVOCATION,
            applied_at: "2026-10-09T00:00:00Z",
        }
    }

    /// The known-answer acknowledgement, built through the payload builder.
    #[test]
    fn a7_revocation_ack_vector() {
        let signed = sign_revocation_ack(&host(), &fields());
        assert_eq!(
            signed.payload,
            r#"{"v":2,"type":"revocation-ack","host":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","keySeq":0,"principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","revocationSha256":"VYCvacXofNMIe3q3-owfpopw8oLdXAOc6KvNKsEtis4","appliedAt":"2026-10-09T00:00:00Z"}"#
        );
        assert_eq!(
            signed.signature,
            "ed25519:waKbDNMuQ460M8gX_H1NKzAMDXF3x4P8hbHoJFPr-pSduvfvQSbXvOcRmK5VcF5F9-sICQmF-pBnACBu9Z-mDg"
        );
        let ack = verify_revocation_ack(&host().public_key(), &signed).expect("verify");
        assert!(ack.acknowledges(REVOCATION));
        assert!(!ack.acknowledges("{}"));
        assert_eq!(ack.device_id, DEVICE);
    }

    #[test]
    fn refuses_another_key_another_kind_and_extra_fields() {
        let signed = sign_revocation_ack(&host(), &fields());
        assert!(
            verify_revocation_ack(&ServerSigner::from_seed([41; 32]).public_key(), &signed)
                .is_err()
        );
        let as_event = statement::sign(
            &host(),
            ServerStatement::HostEvent,
            revocation_ack_payload(&fields()),
        );
        assert!(verify_revocation_ack(&host().public_key(), &as_event).is_err());
        let extra = statement::sign(
            &host(),
            ServerStatement::RevocationAck,
            revocation_ack_payload(&fields()).replace(r#""v":2,"#, r#""v":2,"x":1,"#),
        );
        assert!(verify_revocation_ack(&host().public_key(), &extra).is_err());
        let v1 = statement::sign(
            &host(),
            ServerStatement::RevocationAck,
            revocation_ack_payload(&fields()).replace(r#""v":2,"#, r#""v":1,"#),
        );
        assert!(verify_revocation_ack(&host().public_key(), &v1).is_err());
    }
}
