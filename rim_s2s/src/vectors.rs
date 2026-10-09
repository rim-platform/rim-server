//! Known-answer vectors: RFC 9421 appendix B.2.6, and every value of appendix A of the
//! server-to-server specification, reproduced byte for byte.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::{DateTime, TimeZone, Utc};
use ed25519_dalek::SigningKey;
use rim_identity::{Purpose, StatementVersion, sign, sign_with_context};
use sha2::{Digest, Sha256};

use crate::descriptor::{
    Endpoints, Limits, Registration, RegistrationMode, Role, ServerDescriptor, sign_descriptor,
    verify_descriptor,
};
use crate::http_sig::{
    Component, IncomingRequest, IncomingResponse, KeyId, OutgoingRequest, OutgoingResponse, Param,
    RequestBinding, content_digest, parse_request, parse_response, sign_request, sign_response,
    signature_base, signature_params,
};
use crate::key_log::{
    ServerKeyState, ServerRotationSigner, key_hash, sign_inception, sign_rotation,
};
use crate::signer::ServerSigner;
use crate::statement::{self, ServerStatement};

fn b64u(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn server(seed: u8) -> ServerSigner {
    ServerSigner::from_seed([seed; 32])
}

fn device() -> SigningKey {
    SigningKey::from_bytes(&[20; 32])
}

fn day(d: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, d, 0, 0, 0)
        .single()
        .expect("time")
}

const T: &str = "2026-10-09T00:00:00Z";
const CREATED: i64 = 1_791_504_000;
const PRINCIPAL: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";
const DEVICE: &str = "00000000-0000-0000-0000-000000000001";

const RELAY_ID: &str = "rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c";
const RELAY_KEY_HASH: &str = "8UGftl4tBoxZjUo1UP_-zwUlobQJPwj5kZ0NC1oaAaM";
const HOST_ID: &str = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";

const HOST_INCEPTION: &str = r#"{"v":2,"type":"server-inception","seq":0,"serverKey":"ed25519:6S62BU_pvGgqG887dZ9lqzikz72BxNHzNC5Myc3tiws","recoveryKey":"ed25519:_L44YyQXu1uHXUmjwCJwYzkXxgk_0B67Wf9kFuN6_gw","createdAt":"2026-10-09T00:00:00Z"}"#;

/// A.1: both inceptions, their ids, key hashes, `prev` values and signatures.
#[test]
fn a1_server_inceptions() {
    struct Expected {
        seeds: (u8, u8),
        server_key: &'static str,
        recovery_key: &'static str,
        server_id: &'static str,
        key_hash: &'static str,
        prev: &'static str,
        signature: &'static str,
    }
    let cases = [
        Expected {
            seeds: (30, 130),
            server_key: "ed25519:rNsOKXQ_DMuGhtChBMuW4Fq-_sFTh2XnWVhp99yMSao",
            recovery_key: "ed25519:-gE0epkeBK4PGm2B2yml6k7rWWX1d6ye-0TnsfL7sCg",
            server_id: RELAY_ID,
            key_hash: RELAY_KEY_HASH,
            prev: "dMszB0YhzGkVSf4B-3elynzitpFsvVYojOEW8hXiNKg",
            signature: "ed25519:_oQe2QcbA37vNosXrUVHFf8iIbnaesyNMmPtj827Wx5Q6vt77tyaOYR6JA9e3Z2FjHDRs0uR0XQEj9TMwoIODA",
        },
        Expected {
            seeds: (40, 140),
            server_key: "ed25519:6S62BU_pvGgqG887dZ9lqzikz72BxNHzNC5Myc3tiws",
            recovery_key: "ed25519:_L44YyQXu1uHXUmjwCJwYzkXxgk_0B67Wf9kFuN6_gw",
            server_id: HOST_ID,
            key_hash: "qkSWxxJEL_11l8_dTCGhHc_1Q1pAc2pMirMBGnz7388",
            prev: "oUvCbfqOS1bUTL5gAtene_faqghv8PaPgrRDIJpXLOM",
            signature: "ed25519:Dll_XFAqcMn9Pejfpge4oAGiEH16dAEu8KLjLjSEDPd8UFk3-tmRgzINC2G89uyREl1aDYI5bA-O8XOF7oftAg",
        },
    ];
    for e in cases {
        let (s, r) = (server(e.seeds.0), server(e.seeds.1));
        assert_eq!(s.public_key().to_string(), e.server_key);
        assert_eq!(r.public_key().to_string(), e.recovery_key);
        let inception = sign_inception(&s, &r, day(9));
        assert_eq!(inception.signature, e.signature);
        let state = ServerKeyState::from_inception(&inception).expect("inception");
        assert_eq!(state.server_id, e.server_id);
        assert_eq!(key_hash(&state.server_key), e.key_hash);
        assert_eq!(state.next_prev(), e.prev);
        if e.server_id == HOST_ID {
            assert_eq!(inception.payload, HOST_INCEPTION);
        }
    }
}

fn host_state() -> ServerKeyState {
    ServerKeyState::from_inception(&sign_inception(&server(40), &server(140), day(9)))
        .expect("inception")
}

fn host_descriptor(key_seq: u32, version: u32, issued_at: &str) -> ServerDescriptor {
    ServerDescriptor::new(
        HOST_ID.to_string(),
        key_seq,
        version,
        vec![Role::Host, Role::Relay],
        Endpoints {
            graphql: "https://host.example/api".into(),
            websocket: "wss://host.example/api/ws".into(),
            s2s: Some("https://host.example/s2s/v1".into()),
        },
        vec![79],
        Limits {
            max_body_bytes: 8_388_608,
            clock_skew_seconds: 300,
            max_deliver_items: 256,
        },
        Registration {
            mode: RegistrationMode::Invite,
            pow_bits: None,
        },
        issued_at.to_string(),
    )
}

/// A.2: the host descriptor's exact payload and signature.
#[test]
fn a2_host_descriptor() {
    let signed = sign_descriptor(&server(40), &host_descriptor(0, 1, T)).expect("sign");
    assert_eq!(
        signed.payload,
        r#"{"v":2,"type":"server-descriptor","serverId":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","keySeq":0,"version":1,"roles":["host","relay"],"endpoints":{"graphql":"https://host.example/api","websocket":"wss://host.example/api/ws","s2s":"https://host.example/s2s/v1"},"s2sVersions":[1],"mlsCipherSuites":[79],"limits":{"maxBodyBytes":8388608,"clockSkewSeconds":300,"maxDeliverItems":256},"registration":{"mode":"invite","powBits":null},"issuedAt":"2026-10-09T00:00:00Z"}"#
    );
    assert_eq!(
        signed.signature,
        "ed25519:ZdBDMoUEFhLQMnX7dK_r8xUPa0dNoSHOZqfAHelFegW0PUpluRUX84gzBzp2TtMKku19aCrf4wMXjbemA8b_Bw"
    );
    assert!(verify_descriptor(&host_state(), &signed).is_ok());
}

const DELEGATION: &str = r#"{"v":2,"type":"delegation","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","relay":"rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c","relayKeyHash":"8UGftl4tBoxZjUo1UP_-zwUlobQJPwj5kZ0NC1oaAaM","host":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","workspaceId":"0199d1a0-0000-7000-8000-000000000002","scope":"workspace","nonce":"cHFyc3R1dnd4eXp7fH1-fw","issuedAt":"2026-10-09T00:00:00Z","expiresAt":"2026-10-10T00:00:00Z"}"#;
const DELEGATION_SIGNATURE: &str = "ed25519:aT8j0zeSdSqWySreIIeRokrjVoYuO-3z1k4IcsFN898M2x_in0B9UlaL5hCUAdw-gCisynSQi3b51QQiZdHlCQ";

/// A.3: the delegation's nonce, relay key hash and device signature.
#[test]
fn a3_delegation() {
    assert_eq!(
        b64u(&(0x70u8..0x80).collect::<Vec<_>>()),
        "cHFyc3R1dnd4eXp7fH1-fw"
    );
    assert!(DELEGATION.contains(&format!(
        r#""relayKeyHash":"{}""#,
        key_hash(&server(30).public_key())
    )));
    let signed = sign_with_context(b"rim.delegation.v1\0", &device(), DELEGATION);
    assert_eq!(signed.signature, DELEGATION_SIGNATURE);
}

const REQUEST_NONCE: &str = "YGFiY2RlZmdoaWprbG1ubw";
const TARGET: &str = "https://host.example/s2s/v1/subscriptions";

/// A.4: the relay's signed request and the host's signed response, bases included.
#[test]
fn a4_signed_request_and_response() {
    assert_eq!(b64u(&(0x60u8..0x70).collect::<Vec<_>>()), REQUEST_NONCE);
    let body = serde_json::to_vec(&serde_json::json!({
        "delegations": [{"payload": DELEGATION, "signature": DELEGATION_SIGNATURE}]
    }))
    .expect("json");
    assert_eq!(body.len(), 685);
    let digest = "sha-256=:NLGvVDM7TsvDtemcqNyHnp4tVM9bTzAjg1MbseDLrOc=:";
    assert_eq!(content_digest(&body), digest);

    let relay_key = KeyId {
        server_id: RELAY_ID.into(),
        seq: 0,
    };
    let request = OutgoingRequest {
        method: "POST",
        target_uri: TARGET,
        body: Some(&body),
        content_type: "application/json",
        rim_origin: "https://relay.example",
        created: CREATED,
        expires: CREATED + 300,
        nonce: REQUEST_NONCE,
        key_id: &relay_key,
    };
    let headers = sign_request(&server(30), &request).expect("sign");
    let params = r#"("@method" "@target-uri" "content-digest" "content-type" "rim-origin");created=1791504000;expires=1791504300;nonce="YGFiY2RlZmdoaWprbG1ubw";keyid="rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c#0";alg="ed25519";tag="rim-s2s-v1""#;
    assert_eq!(headers.signature_input, format!("sig1={params}"));
    assert_eq!(
        headers.signature,
        "sig1=:N1zyrK5mP9qxsSJOKAriCI38lQHL2H8FiKprKMqOay1+XFgIaldQw7J5zILaG6/hWcw6ESUC4HGY/bJ1O+QMDg==:"
    );
    let printed_base = format!(
        "\"@method\": POST\n\"@target-uri\": {TARGET}\n\"content-digest\": {digest}\n\"content-type\": application/json\n\"rim-origin\": https://relay.example\n\"@signature-params\": {params}"
    );
    let resigned = STANDARD.encode(server(30).sign_bytes(printed_base.as_bytes()));
    assert_eq!(headers.signature, format!("sig1=:{resigned}:"));
    let pending = parse_request(
        &IncomingRequest {
            method: "POST",
            target_uri: TARGET,
            content_digest: Some(digest),
            content_type: Some("application/json"),
            rim_origin: Some("https://relay.example"),
            signature_input: Some(&headers.signature_input),
            signature: Some(&headers.signature),
        },
        CREATED,
    )
    .expect("parse");
    pending.check_digest(&body).expect("digest");
    pending.verify(&server(30).public_key()).expect("verify");

    let response_body = br#"{"subscriptions":[{"workspaceId":"0199d1a0-0000-7000-8000-000000000002","status":"active","expiresAt":"2026-10-10T00:00:00Z"}]}"#;
    let response_digest = "sha-256=:BBeCXY4Vnpnqik7XJ8TNUUC/Y392lt0Zhhcz9uwijB8=:";
    let host_key = KeyId {
        server_id: HOST_ID.into(),
        seq: 0,
    };
    let binding = RequestBinding {
        method: "POST",
        target_uri: TARGET,
        content_digest: Some(digest),
        nonce: REQUEST_NONCE,
    };
    let response = sign_response(
        &server(40),
        &OutgoingResponse {
            status: 200,
            body: response_body,
            content_type: "application/json",
            request: binding,
            created: CREATED + 1,
            key_id: &host_key,
        },
    )
    .expect("sign");
    assert_eq!(response.content_digest.as_deref(), Some(response_digest));
    let response_params = r#"("@status" "content-digest" "content-type" "rim-request-nonce" "@method";req "@target-uri";req "content-digest";req);created=1791504001;keyid="rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw#0";alg="ed25519";tag="rim-s2s-v1""#;
    assert_eq!(response.signature_input, format!("sig1={response_params}"));
    assert_eq!(
        response.signature,
        "sig1=:rGIqqwBn079G9OciB/u4fSIro9i6S0P97QNxFSo8UxPvXJwywOS4z5NrmPIYNnr2MKxK2dSzsBl0eU97wQFGAw==:"
    );
    let printed_base = format!(
        "\"@status\": 200\n\"content-digest\": {response_digest}\n\"content-type\": application/json\n\"rim-request-nonce\": {REQUEST_NONCE}\n\"@method\";req: POST\n\"@target-uri\";req: {TARGET}\n\"content-digest\";req: {digest}\n\"@signature-params\": {response_params}"
    );
    let resigned = STANDARD.encode(server(40).sign_bytes(printed_base.as_bytes()));
    assert_eq!(response.signature, format!("sig1=:{resigned}:"));
    let pending = parse_response(
        &IncomingResponse {
            status: 200,
            content_digest: Some(response_digest),
            content_type: Some("application/json"),
            rim_request_nonce: Some(REQUEST_NONCE),
            signature_input: Some(&response.signature_input),
            signature: Some(&response.signature),
        },
        &binding,
        CREATED + 1,
    )
    .expect("parse");
    pending.check_digest(response_body).expect("digest");
    pending.verify(&server(40).public_key()).expect("verify");
}

fn check_server_statement(signer: u8, kind: ServerStatement, payload: &str, signature: &str) {
    let signed = statement::sign(&server(signer), kind, payload);
    assert_eq!(signed.signature, signature);
    statement::verify(kind, &server(signer).public_key(), &signed).expect("verify");
}

/// A.5: the host event's signature.
#[test]
fn a5_host_event() {
    check_server_statement(
        40,
        ServerStatement::HostEvent,
        r#"{"v":2,"type":"host-event","host":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","keySeq":0,"workspaceId":"0199d1a0-0000-7000-8000-000000000002","stream":"c:0199d1a0-0000-7000-8000-000000000003","seq":42,"event":{"type":"MessageDeleted","workspace_id":"0199d1a0-0000-7000-8000-000000000002","conversation_id":"0199d1a0-0000-7000-8000-000000000003","conversation_type":"Channel","message_id":"0199d1a0-0000-7000-8000-000000000004","thread_id":null},"issuedAt":"2026-10-09T00:00:00Z"}"#,
        "ed25519:CPSwWGrkjbauuToACdrAuJL4L6mv1XpGTPy_XKjkymGwzqx6pyW-j6V52HkhpPlXx82tXRbQTI-GsH9N4N-zBg",
    );
}

/// A.6: the device-signed request, its header, and the host-signed response.
#[test]
fn a6_device_request_and_host_response() {
    let body = br#"{"query":"mutation($id:ID!){deleteMessage(messageId:$id)}","variables":{"id":"0199d1a0-0000-7000-8000-000000000004"}}"#;
    assert_eq!(body.len(), 117);
    let body_hash = b64u(&sha(body));
    assert_eq!(body_hash, "a2PtmXdQU8dMqDduw8cQymbwEmFEilShfZfDDArTcwU");
    let payload = format!(
        r#"{{"v":2,"type":"device-request","principalId":"{PRINCIPAL}","deviceId":"{DEVICE}","host":"{HOST_ID}","method":"POST","path":"/api","seq":1791504000000000,"bodySha256":"{body_hash}","issuedAt":"{T}"}}"#
    );
    let signed = sign_with_context(b"rim.device-request.v1\0", &device(), payload.clone());
    assert_eq!(
        signed.signature,
        "ed25519:IoWInJdss89VdGgCEnWvLRCAHZ--5mrIzRiDV0thfCwLOVOgyEZzaGG7CGBfgKqnnJiJvzHMxaCZYFiEMiSbCg"
    );
    assert_eq!(
        b64u(payload.as_bytes()),
        "eyJ2IjoyLCJ0eXBlIjoiZGV2aWNlLXJlcXVlc3QiLCJwcmluY2lwYWxJZCI6InJpbXAxX3R4RExibmlwaTVpM2tDSnRKd0JPVWl4enZfTVk4T25VMG1jeS0tVHlqZTAiLCJkZXZpY2VJZCI6IjAwMDAwMDAwLTAwMDAtMDAwMC0wMDAwLTAwMDAwMDAwMDAwMSIsImhvc3QiOiJyaW1zMV9PRlBETlhUVjFzcGxDY0tjZ3oxdldZajZNZy1sYlVvbGxIUXdYVGtZRUJ3IiwibWV0aG9kIjoiUE9TVCIsInBhdGgiOiIvYXBpIiwic2VxIjoxNzkxNTA0MDAwMDAwMDAwLCJib2R5U2hhMjU2IjoiYTJQdG1YZFFVOGRNcURkdXc4Y1F5bWJ3RW1GRWlsU2hmWmZEREFyVGN3VSIsImlzc3VlZEF0IjoiMjAyNi0xMC0wOVQwMDowMDowMFoifQ"
    );

    let request_hash = b64u(&sha(payload.as_bytes()));
    assert_eq!(request_hash, "USMHYKhT_TDTgBiA_mrWvOz3-RZLlohDKUQREkVxMCY");
    let answer_hash = b64u(&sha(br#"{"data":{"deleteMessage":true}}"#));
    assert_eq!(answer_hash, "6lPwX_33fnLq0QTkKknG-F-tK81icmIjQKnN0eS8JA0");
    check_server_statement(
        40,
        ServerStatement::HostResponse,
        &format!(
            r#"{{"v":2,"type":"host-response","host":"{HOST_ID}","keySeq":0,"requestSha256":"{request_hash}","status":200,"bodySha256":"{answer_hash}"}}"#
        ),
        "ed25519:f-vulCqj3GgD3dI4zNAeCMs1JGyDwkzbkcLhk0uXXMz4NCJ-az1ZVKWxgpbqNMWTzMhSKtnFPhwMHmATk69zDg",
    );
}

/// A.7: the self-revocation and the host's acknowledgement.
#[test]
fn a7_revocation_and_acknowledgement() {
    let revocation = format!(
        r#"{{"v":2,"type":"device-revoke","principalId":"{PRINCIPAL}","deviceId":"{DEVICE}","signer":"device","issuedAt":"{T}"}}"#
    );
    let signed = sign(
        Purpose::DeviceRevoke,
        StatementVersion::V2,
        &device(),
        revocation.clone(),
    );
    assert_eq!(
        signed.signature,
        "ed25519:kCcxKVwjdFvYSC3xx3yHWgO5gWwwq9wpC8bzQs1NPn9bKhAUJSBdLawiI1bWkNDDOdeW_x7AEm0fafPf38D3Bw"
    );
    let revocation_hash = b64u(&sha(revocation.as_bytes()));
    assert_eq!(
        revocation_hash,
        "VYCvacXofNMIe3q3-owfpopw8oLdXAOc6KvNKsEtis4"
    );
    check_server_statement(
        40,
        ServerStatement::RevocationAck,
        &format!(
            r#"{{"v":2,"type":"revocation-ack","host":"{HOST_ID}","keySeq":0,"principalId":"{PRINCIPAL}","deviceId":"{DEVICE}","revocationSha256":"{revocation_hash}","appliedAt":"{T}"}}"#
        ),
        "ed25519:waKbDNMuQ460M8gX_H1NKzAMDXF3x4P8hbHoJFPr-pSduvfvQSbXvOcRmK5VcF5F9-sICQmF-pBnACBu9Z-mDg",
    );
}

/// A.8: the smallest proof-of-work counter at 16 bits for key-identity appendix B's inception.
#[test]
fn a8_proof_of_work() {
    let inception = r#"{"v":2,"type":"inception","seq":0,"identityKey":"ed25519:iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w","recoveryKey":"ed25519:1i8Bah79Hk_feT60LNhEceG6nwzwTRKHtcxx9hYofLg","createdAt":"2026-10-09T00:00:00Z"}"#;
    assert_eq!(
        rim_identity::principal_id_for_inception(inception),
        PRINCIPAL
    );
    let hash_for = |counter: u64| {
        let mut h = Sha256::new();
        h.update(b"rim.pow.v1\0");
        h.update(RELAY_ID.as_bytes());
        h.update(b"\0");
        h.update(inception.as_bytes());
        h.update(counter.to_be_bytes());
        <[u8; 32]>::from(h.finalize())
    };
    let counter = (0u64..)
        .find(|c| {
            let h = hash_for(*c);
            h[0] == 0 && h[1] == 0
        })
        .expect("counter");
    assert_eq!(counter, 37283);
    assert_eq!(b64u(&counter.to_be_bytes()), "AAAAAAAAkaM");
    let hex: String = hash_for(counter)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(
        hex,
        "00002f63d137749fda27201ccf9d282a4ed846287d49c55fea89a47617e78ed6"
    );
}

/// RFC 9421 appendix B.2.6, "Signing a Request Using ed25519", through this crate's parameter
/// serialization, base construction and signing.
#[test]
fn rfc9421_b26_ed25519() {
    // The RFC's test-key-ed25519 (appendix B.1.4): PKCS#8 with the seed in the last 32 bytes,
    // SubjectPublicKeyInfo with the key in the last 32.
    let der = STANDARD
        .decode("MC4CAQAwBQYDK2VwBCIEIJ+DYvh6SEqVTm50DFtMDoQikTmiCqirVv9mWG9qfSnF")
        .expect("der");
    let seed: [u8; 32] = der[16..].try_into().expect("seed");
    let signer = ServerSigner::from_seed(seed);
    let public = STANDARD
        .decode("MCowBQYDK2VwAyEAJrQLj5P/89iXES9+vFgrIy29clF9CC/oPPsw3c5D0bs=")
        .expect("spki");
    assert_eq!(signer.public_key().key_bytes(), public[12..]);

    let components = [
        Component::new("date"),
        Component::new("@method"),
        Component::new("@path"),
        Component::new("@authority"),
        Component::new("content-type"),
        Component::new("content-length"),
    ];
    let params = signature_params(
        &components,
        &[
            ("created", Param::Int(1_618_884_473)),
            ("keyid", Param::Str("test-key-ed25519".into())),
        ],
    )
    .expect("params");
    assert_eq!(
        format!("sig-b26={params}"),
        r#"sig-b26=("date" "@method" "@path" "@authority" "content-type" "content-length");created=1618884473;keyid="test-key-ed25519""#
    );
    let values = [
        "Tue, 20 Apr 2021 02:07:55 GMT",
        "POST",
        "/foo",
        "example.com",
        "application/json",
        "18",
    ];
    let lines: Vec<(Component, &str)> = components.iter().copied().zip(values).collect();
    let base = signature_base(&lines, &params);
    assert_eq!(
        base,
        "\"date\": Tue, 20 Apr 2021 02:07:55 GMT\n\"@method\": POST\n\"@path\": /foo\n\"@authority\": example.com\n\"content-type\": application/json\n\"content-length\": 18\n\"@signature-params\": (\"date\" \"@method\" \"@path\" \"@authority\" \"content-type\" \"content-length\");created=1618884473;keyid=\"test-key-ed25519\""
    );
    let signature = signer.sign_bytes(base.as_bytes());
    assert_eq!(
        STANDARD.encode(&signature),
        "wqcAqbmYJ2ji2glfAMaRy4gruYYnx2nEFN2HN6jrnDnQCK1u02Gb04v9EDgwUPiu4A0w6vuQv5lIp5WPpBKRCw=="
    );
    signer
        .public_key()
        .verify_bytes(base.as_bytes(), &signature)
        .expect("verify");
}

/// The rotation vector: the host rotates from seed 40 to seed 41 (`signer: server`, recovery
/// key unchanged) and re-signs its descriptor at `keySeq` 1, `version` 2.
#[test]
fn host_rotation_and_rotated_descriptor() {
    let state = host_state();
    let rotation = sign_rotation(
        &state,
        &server(40),
        ServerRotationSigner::Server,
        &server(41).public_key(),
        &server(140).public_key(),
        day(10),
    );
    assert_eq!(
        rotation.payload,
        r#"{"v":2,"type":"server-rotate","serverId":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","seq":1,"prev":"oUvCbfqOS1bUTL5gAtene_faqghv8PaPgrRDIJpXLOM","serverKey":"ed25519:-kg0FH9uaQw2k-_2EzYEZAPNiuKhTzGzxAc1hWkjlWU","recoveryKey":"ed25519:_L44YyQXu1uHXUmjwCJwYzkXxgk_0B67Wf9kFuN6_gw","signer":"server","createdAt":"2026-10-10T00:00:00Z"}"#
    );
    assert_eq!(
        rotation.signature,
        "ed25519:cysmxfZAsbKJKvFGkftqHCqAzySvf4MMD9PTimeNavN_Oi91Zt-lfcvCswkb08SnVDrS-iMWK1WT1n7iY_fZAA"
    );
    let rotated = state.apply(&rotation).expect("rotate");
    assert_eq!(
        key_hash(&rotated.server_key),
        "fvKh1r_va0lqRvlpGojY2V1JzJMQeSESlNpO4OXw6Ak"
    );
    assert_eq!(
        rotated.next_prev(),
        "sU02HLfXJNvkpkHn28DO3JdBxQpmTr3Dl4y94zIY9hU"
    );

    let descriptor =
        sign_descriptor(&server(41), &host_descriptor(1, 2, "2026-10-10T00:00:00Z")).expect("sign");
    assert_eq!(
        descriptor.payload,
        r#"{"v":2,"type":"server-descriptor","serverId":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","keySeq":1,"version":2,"roles":["host","relay"],"endpoints":{"graphql":"https://host.example/api","websocket":"wss://host.example/api/ws","s2s":"https://host.example/s2s/v1"},"s2sVersions":[1],"mlsCipherSuites":[79],"limits":{"maxBodyBytes":8388608,"clockSkewSeconds":300,"maxDeliverItems":256},"registration":{"mode":"invite","powBits":null},"issuedAt":"2026-10-10T00:00:00Z"}"#
    );
    assert_eq!(
        descriptor.signature,
        "ed25519:Yuy9VdOS78e8fL4sMDT9PHmBkZcuzeJEvLlfdD8dRhDjJeJiivU4FcatDrlwEKaB3l9Beado7Qi6_33D7UN0AA"
    );
    verify_descriptor(&rotated, &descriptor).expect("verify");
    // The A.2 descriptor, signed at seq 0, no longer verifies against the rotated log.
    let old = sign_descriptor(&server(40), &host_descriptor(0, 1, T)).expect("sign");
    assert!(verify_descriptor(&rotated, &old).is_err());
}
