//! Statements a host signs for relays and clients: `host-event` (an event sent to relays) and
//! `host-response` (the answer to a forwarded device request).

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rim_identity::{PublicKey, SignedStatement};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{StatementError, StatementResult};
use crate::signer::ServerSigner;
use crate::statement::{self, STATEMENT_VERSION, ServerStatement, parse_payload};

/// Unpadded base64url SHA-256 of `bytes`.
pub fn sha256_b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(bytes))
}

/// A JSON string literal.
fn quoted(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

fn check_header(v: u32, kind: &str, expected: &'static str) -> StatementResult<()> {
    if v != STATEMENT_VERSION {
        return Err(rim_identity::IdentityError::UnsupportedVersion(v).into());
    }
    if kind != expected {
        return Err(rim_identity::IdentityError::WrongType { expected }.into());
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawHostEvent {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    host: String,
    key_seq: u32,
    workspace_id: String,
    stream: String,
    seq: Option<u64>,
    event: serde_json::Value,
    issued_at: String,
}

/// A verified host event.
#[derive(Debug, Clone, PartialEq)]
pub struct HostEvent {
    pub host: String,
    pub key_seq: u32,
    pub workspace_id: String,
    /// `c:<conversationId>`, `u:<principalId>`, `w` or `e`.
    pub stream: String,
    /// The position in the stream; `None` on the `e` stream.
    pub seq: Option<u64>,
    /// The event as clients parse it.
    pub event: serde_json::Value,
    pub issued_at: String,
}

/// The fields of a host event to sign. `event_json` is the event's JSON object text, embedded
/// as it is.
#[derive(Debug, Clone, Copy)]
pub struct HostEventFields<'a> {
    pub host: &'a str,
    pub key_seq: u32,
    pub workspace_id: &'a str,
    pub stream: &'a str,
    pub seq: Option<u64>,
    pub event_json: &'a str,
    pub issued_at: &'a str,
}

/// The payload text of a host event, in its fixed field order.
pub fn host_event_payload(fields: &HostEventFields<'_>) -> String {
    let seq = fields
        .seq
        .map_or_else(|| "null".to_string(), |s| s.to_string());
    format!(
        r#"{{"v":2,"type":"host-event","host":{},"keySeq":{},"workspaceId":{},"stream":{},"seq":{seq},"event":{},"issuedAt":{}}}"#,
        quoted(fields.host),
        fields.key_seq,
        quoted(fields.workspace_id),
        quoted(fields.stream),
        fields.event_json,
        quoted(fields.issued_at),
    )
}

/// Sign a host event.
pub fn sign_host_event(signer: &ServerSigner, fields: &HostEventFields<'_>) -> SignedStatement {
    statement::sign(
        signer,
        ServerStatement::HostEvent,
        host_event_payload(fields),
    )
}

/// Read a host event's fields without checking its signature, to find the key it names.
pub fn read_host_event(statement: &SignedStatement) -> StatementResult<HostEvent> {
    let raw: RawHostEvent = parse_payload(ServerStatement::HostEvent, &statement.payload)?;
    check_header(raw.v, &raw.kind, "host-event")?;
    if !raw.event.is_object() {
        return Err(StatementError::Invalid(
            "a host event's event must be an object".to_string(),
        ));
    }
    if raw.stream == "e" && raw.seq.is_some() || raw.stream != "e" && raw.seq.is_none() {
        return Err(StatementError::Invalid(
            "seq is null on the e stream and set on every other".to_string(),
        ));
    }
    Ok(HostEvent {
        host: raw.host,
        key_seq: raw.key_seq,
        workspace_id: raw.workspace_id,
        stream: raw.stream,
        seq: raw.seq,
        event: raw.event,
        issued_at: raw.issued_at,
    })
}

/// Check a host event's signature by `key` (the host's key at the event's `keySeq`) and read it.
pub fn verify_host_event(
    key: &PublicKey,
    statement: &SignedStatement,
) -> StatementResult<HostEvent> {
    let event = read_host_event(statement)?;
    statement::verify(ServerStatement::HostEvent, key, statement)?;
    Ok(event)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawHostResponse {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    host: String,
    key_seq: u32,
    request_sha256: String,
    status: u16,
    body_sha256: String,
}

/// A verified host response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostResponse {
    pub host: String,
    pub key_seq: u32,
    /// Hash of the device-request payload text the response answers.
    pub request_sha256: String,
    pub status: u16,
    pub body_sha256: String,
}

impl HostResponse {
    /// Check that this response answers `device_request_payload` with `status` and `body`.
    pub fn check(&self, device_request_payload: &str, status: u16, body: &[u8]) -> bool {
        self.request_sha256 == sha256_b64(device_request_payload.as_bytes())
            && self.status == status
            && self.body_sha256 == sha256_b64(body)
    }
}

/// The payload text of a host response.
pub fn host_response_payload(
    host: &str,
    key_seq: u32,
    device_request_payload: &str,
    status: u16,
    body: &[u8],
) -> String {
    format!(
        r#"{{"v":2,"type":"host-response","host":{},"keySeq":{key_seq},"requestSha256":"{}","status":{status},"bodySha256":"{}"}}"#,
        quoted(host),
        sha256_b64(device_request_payload.as_bytes()),
        sha256_b64(body),
    )
}

/// Sign the answer to a forwarded device request.
pub fn sign_host_response(
    signer: &ServerSigner,
    host: &str,
    key_seq: u32,
    device_request_payload: &str,
    status: u16,
    body: &[u8],
) -> SignedStatement {
    statement::sign(
        signer,
        ServerStatement::HostResponse,
        host_response_payload(host, key_seq, device_request_payload, status, body),
    )
}

/// Check a host response's signature by `key` and read it.
pub fn verify_host_response(
    key: &PublicKey,
    statement: &SignedStatement,
) -> StatementResult<HostResponse> {
    let raw: RawHostResponse = parse_payload(ServerStatement::HostResponse, &statement.payload)?;
    check_header(raw.v, &raw.kind, "host-response")?;
    statement::verify(ServerStatement::HostResponse, key, statement)?;
    Ok(HostResponse {
        host: raw.host,
        key_seq: raw.key_seq,
        request_sha256: raw.request_sha256,
        status: raw.status,
        body_sha256: raw.body_sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST_ID: &str = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";
    const EVENT: &str = r#"{"type":"MessageDeleted","workspace_id":"0199d1a0-0000-7000-8000-000000000002","conversation_id":"0199d1a0-0000-7000-8000-000000000003","conversation_type":"Channel","message_id":"0199d1a0-0000-7000-8000-000000000004","thread_id":null}"#;

    fn host() -> ServerSigner {
        ServerSigner::from_seed([40; 32])
    }

    fn fields(stream: &str, seq: Option<u64>) -> HostEventFields<'_> {
        HostEventFields {
            host: HOST_ID,
            key_seq: 0,
            workspace_id: "0199d1a0-0000-7000-8000-000000000002",
            stream,
            seq,
            event_json: EVENT,
            issued_at: "2026-10-09T00:00:00Z",
        }
    }

    /// The known-answer host event, built through the payload builder.
    #[test]
    fn a5_host_event_vector() {
        let signed = sign_host_event(
            &host(),
            &fields("c:0199d1a0-0000-7000-8000-000000000003", Some(42)),
        );
        assert_eq!(
            signed.payload,
            r#"{"v":2,"type":"host-event","host":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","keySeq":0,"workspaceId":"0199d1a0-0000-7000-8000-000000000002","stream":"c:0199d1a0-0000-7000-8000-000000000003","seq":42,"event":{"type":"MessageDeleted","workspace_id":"0199d1a0-0000-7000-8000-000000000002","conversation_id":"0199d1a0-0000-7000-8000-000000000003","conversation_type":"Channel","message_id":"0199d1a0-0000-7000-8000-000000000004","thread_id":null},"issuedAt":"2026-10-09T00:00:00Z"}"#
        );
        assert_eq!(
            signed.signature,
            "ed25519:CPSwWGrkjbauuToACdrAuJL4L6mv1XpGTPy_XKjkymGwzqx6pyW-j6V52HkhpPlXx82tXRbQTI-GsH9N4N-zBg"
        );
        let event = verify_host_event(&host().public_key(), &signed).expect("verify");
        assert_eq!(event.seq, Some(42));
        assert_eq!(event.event["type"], "MessageDeleted");
        assert!(
            verify_host_event(&ServerSigner::from_seed([41; 32]).public_key(), &signed).is_err()
        );
    }

    /// The known-answer host response, built through the payload builder.
    #[test]
    fn a6_host_response_vector() {
        let request = r#"{"v":2,"type":"device-request","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","host":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","method":"POST","path":"/api","seq":1791504000000000,"bodySha256":"a2PtmXdQU8dMqDduw8cQymbwEmFEilShfZfDDArTcwU","issuedAt":"2026-10-09T00:00:00Z"}"#;
        let body = br#"{"data":{"deleteMessage":true}}"#;
        let signed = sign_host_response(&host(), HOST_ID, 0, request, 200, body);
        assert_eq!(
            signed.payload,
            r#"{"v":2,"type":"host-response","host":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","keySeq":0,"requestSha256":"USMHYKhT_TDTgBiA_mrWvOz3-RZLlohDKUQREkVxMCY","status":200,"bodySha256":"6lPwX_33fnLq0QTkKknG-F-tK81icmIjQKnN0eS8JA0"}"#
        );
        assert_eq!(
            signed.signature,
            "ed25519:f-vulCqj3GgD3dI4zNAeCMs1JGyDwkzbkcLhk0uXXMz4NCJ-az1ZVKWxgpbqNMWTzMhSKtnFPhwMHmATk69zDg"
        );
        let response = verify_host_response(&host().public_key(), &signed).expect("verify");
        assert!(response.check(request, 200, body));
        assert!(!response.check(request, 500, body));
        assert!(!response.check(request, 200, b"{}"));
        assert!(!response.check("{}", 200, body));
    }

    #[test]
    fn host_statements_refuse_their_other_kind_and_bad_seq() {
        let event = sign_host_event(&host(), &fields("w", Some(1)));
        assert!(verify_host_response(&host().public_key(), &event).is_err());
        let ephemeral = sign_host_event(&host(), &fields("e", None));
        assert_eq!(
            verify_host_event(&host().public_key(), &ephemeral)
                .expect("verify")
                .seq,
            None
        );
        let missing = sign_host_event(&host(), &fields("w", None));
        assert!(verify_host_event(&host().public_key(), &missing).is_err());
        let extra = statement::sign(
            &host(),
            ServerStatement::HostEvent,
            host_event_payload(&fields("w", Some(1))).replace(r#""v":2,"#, r#""v":2,"x":1,"#),
        );
        assert!(verify_host_event(&host().public_key(), &extra).is_err());
    }
}
