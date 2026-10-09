use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::encoding::{PublicKey, b64, sha256};
use crate::error::{IdentityError, IdentityResult};
use crate::statement::{Purpose, SignedStatement, StatementVersion, check_header, parse, verify};

/// Largest `seq` a device request may carry: below 2^53, so it survives a JSON number in every
/// client.
pub const MAX_DEVICE_REQUEST_SEQ: u64 = (1 << 53) - 1;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawDeviceRequest {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    principal_id: String,
    device_id: Uuid,
    host: String,
    method: String,
    path: String,
    seq: u64,
    body_sha256: String,
    #[allow(dead_code)]
    issued_at: DateTime<Utc>,
}

/// The fields of a device-signed request to a space hosted elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRequest {
    pub principal_id: String,
    pub device_id: Uuid,
    /// The host's server id.
    pub host: String,
    pub method: String,
    /// The host-relative path, with any query.
    pub path: String,
    pub seq: u64,
    /// Unpadded base64url SHA-256 of the exact body bytes.
    pub body_sha256: String,
}

impl DeviceRequest {
    /// Check that this request names `host` and is the one forwarded: same method, path and
    /// body.
    pub fn check_forwarded(
        &self,
        host: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> IdentityResult<()> {
        if self.host != host {
            return Err(IdentityError::Invalid(
                "the device request names another host".to_string(),
            ));
        }
        if self.method != method || self.path != path {
            return Err(IdentityError::Invalid(
                "the device request names another method or path".to_string(),
            ));
        }
        if self.body_sha256 != body_sha256(body) {
            return Err(IdentityError::Invalid(
                "the device request names another body".to_string(),
            ));
        }
        Ok(())
    }
}

/// A device request whose signature has not been checked yet. The verifier looks the device
/// key up by `principal_id` and `device_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedDeviceRequest {
    pub principal_id: String,
    pub device_id: Uuid,
    request: DeviceRequest,
    statement: SignedStatement,
}

impl UnverifiedDeviceRequest {
    /// Parse a device request without checking its signature. Only version 2 is accepted.
    pub fn read(statement: &SignedStatement) -> IdentityResult<Self> {
        let raw: RawDeviceRequest = parse(Purpose::DeviceRequest, &statement.payload)?;
        let version = check_header(raw.v, &raw.kind, "device-request")?;
        if version != StatementVersion::V2 {
            return Err(IdentityError::UnsupportedVersion(raw.v));
        }
        if raw.seq > MAX_DEVICE_REQUEST_SEQ {
            return Err(IdentityError::Invalid("seq must be below 2^53".to_string()));
        }
        if raw.method != "GET" && raw.method != "POST" {
            return Err(IdentityError::Invalid(
                "method must be GET or POST".to_string(),
            ));
        }
        if !raw.path.starts_with('/') {
            return Err(IdentityError::Invalid("path must start with /".to_string()));
        }
        Ok(Self {
            principal_id: raw.principal_id.clone(),
            device_id: raw.device_id,
            request: DeviceRequest {
                principal_id: raw.principal_id,
                device_id: raw.device_id,
                host: raw.host,
                method: raw.method,
                path: raw.path,
                seq: raw.seq,
                body_sha256: raw.body_sha256,
            },
            statement: statement.clone(),
        })
    }

    /// Check the signature by the device key.
    pub fn verify(self, device_key: &PublicKey) -> IdentityResult<DeviceRequest> {
        verify(
            Purpose::DeviceRequest,
            StatementVersion::V2,
            device_key,
            &self.statement,
        )?;
        Ok(self.request)
    }
}

/// Unpadded base64url SHA-256 of `body`, as a device request's `bodySha256`.
pub fn body_sha256(body: &[u8]) -> String {
    b64(&sha256(body))
}

/// A JSON string literal.
fn quoted(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

/// The payload text of a device request, in the field order clients write.
#[allow(clippy::too_many_arguments)]
pub fn device_request_payload(
    principal_id: &str,
    device_id: Uuid,
    host: &str,
    method: &str,
    path: &str,
    seq: u64,
    body: &[u8],
    issued_at: &str,
) -> String {
    format!(
        r#"{{"v":2,"type":"device-request","principalId":{},"deviceId":"{device_id}","host":{},"method":{},"path":{},"seq":{seq},"bodySha256":"{}","issuedAt":{}}}"#,
        quoted(principal_id),
        quoted(host),
        quoted(method),
        quoted(path),
        body_sha256(body),
        quoted(issued_at),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statement::{sign, sign_with_context};
    use crate::testing::{TEST_TIME, signing_key};

    const PRINCIPAL: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";
    const HOST: &str = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";

    fn payload(seq: u64, body: &[u8]) -> String {
        device_request_payload(
            PRINCIPAL,
            Uuid::from_u128(1),
            HOST,
            "POST",
            "/api",
            seq,
            body,
            TEST_TIME,
        )
    }

    fn signed(text: String) -> SignedStatement {
        sign_with_context(Purpose::DeviceRequest.context(), &signing_key(20), text)
    }

    /// A.6 of the server-to-server specification.
    #[test]
    fn a6_device_request_vector() {
        let body = br#"{"query":"mutation($id:ID!){deleteMessage(messageId:$id)}","variables":{"id":"0199d1a0-0000-7000-8000-000000000004"}}"#;
        let exact = r#"{"v":2,"type":"device-request","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","host":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","method":"POST","path":"/api","seq":1791504000000000,"bodySha256":"a2PtmXdQU8dMqDduw8cQymbwEmFEilShfZfDDArTcwU","issuedAt":"2026-10-09T00:00:00Z"}"#;
        assert_eq!(payload(1_791_504_000_000_000, body), exact);
        let statement = signed(exact.to_string());
        assert_eq!(
            statement.signature,
            "ed25519:IoWInJdss89VdGgCEnWvLRCAHZ--5mrIzRiDV0thfCwLOVOgyEZzaGG7CGBfgKqnnJiJvzHMxaCZYFiEMiSbCg"
        );
        let request = UnverifiedDeviceRequest::read(&statement)
            .expect("read")
            .verify(&PublicKey::from(&signing_key(20)))
            .expect("verify");
        assert_eq!(request.seq, 1_791_504_000_000_000);
        request
            .check_forwarded(HOST, "POST", "/api", body)
            .expect("forwarded unchanged");
        assert!(
            request
                .check_forwarded(HOST, "POST", "/api", b"{}")
                .is_err()
        );
        assert!(request.check_forwarded(HOST, "GET", "/api", body).is_err());
        assert!(
            request
                .check_forwarded(HOST, "POST", "/api/x", body)
                .is_err()
        );
        assert!(
            request
                .check_forwarded("rims1_other", "POST", "/api", body)
                .is_err()
        );
    }

    #[test]
    fn refuses_version_1_unknown_fields_wrong_type_and_big_seq() {
        let key = signing_key(20);
        let v1 = sign(
            Purpose::DeviceRequest,
            StatementVersion::V1,
            &key,
            payload(1, b"").replace(r#""v":2"#, r#""v":1"#),
        );
        assert_eq!(
            UnverifiedDeviceRequest::read(&v1),
            Err(IdentityError::UnsupportedVersion(1))
        );
        let extra = signed(payload(1, b"").replace(r#""v":2,"#, r#""v":2,"extra":1,"#));
        assert!(matches!(
            UnverifiedDeviceRequest::read(&extra),
            Err(IdentityError::Malformed(_))
        ));
        let login = signed(payload(1, b"").replace("device-request", "login"));
        assert_eq!(
            UnverifiedDeviceRequest::read(&login),
            Err(IdentityError::WrongType {
                expected: "device-request"
            })
        );
        let big = signed(payload(MAX_DEVICE_REQUEST_SEQ + 1, b""));
        assert!(matches!(
            UnverifiedDeviceRequest::read(&big),
            Err(IdentityError::Invalid(_))
        ));
        let put = signed(payload(1, b"").replace(r#""POST""#, r#""PUT""#));
        assert!(matches!(
            UnverifiedDeviceRequest::read(&put),
            Err(IdentityError::Invalid(_))
        ));
    }

    #[test]
    fn refuses_another_key_and_another_context() {
        let statement = signed(payload(7, b"{}"));
        assert_eq!(
            UnverifiedDeviceRequest::read(&statement)
                .expect("read")
                .verify(&PublicKey::from(&signing_key(21))),
            Err(IdentityError::BadSignature)
        );
        let as_login = sign_with_context(
            Purpose::Login.context(),
            &signing_key(20),
            payload(7, b"{}"),
        );
        assert_eq!(
            UnverifiedDeviceRequest::read(&as_login)
                .expect("read")
                .verify(&PublicKey::from(&signing_key(20))),
            Err(IdentityError::BadSignature)
        );
    }
}
