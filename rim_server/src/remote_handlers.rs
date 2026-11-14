//! `GET|POST /api/remote/<hostServerId>/<path>`: a local user's request to a space hosted on
//! another server. The relay checks the session and the device request, then forwards the body
//! unchanged to the host's `/s2s/v1/forward/<path>` and returns the host's answer unchanged.

use std::sync::Arc;

use actix_web::http::{StatusCode, header};
use actix_web::{HttpRequest, HttpResponse, web};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use percent_encoding::percent_decode_str;
use rim_s2s::Role;
use rim_services::auth::service::RequestAuthenticator;
use rim_services::s2s::dto::{S2sMethod, S2sRequest, S2sResponse};

use crate::body::{BodyError, read_bounded};
use crate::file_handlers::{SANDBOX_CSP, map_service_error_to_response, require_csrf};
use crate::graphql_support::extract_auth;
use crate::s2s::S2sState;
use crate::s2s::forward::{
    RIM_DEVICE_REQUEST, RIM_DEVICE_SIGNATURE, RIM_HOST_RESPONSE, RIM_HOST_SIGNATURE,
};

/// Largest forwarded body.
const MAX_FORWARD_BODY_BYTES: usize = 32 * 1024 * 1024;

fn error(status: StatusCode, code: &str, message: &str) -> HttpResponse {
    HttpResponse::build(status).json(serde_json::json!({ "error": code, "message": message }))
}

fn header(req: &HttpRequest, name: &str) -> Option<String> {
    req.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

async fn read_body(req: &HttpRequest, payload: web::Payload) -> Result<Vec<u8>, HttpResponse> {
    read_bounded(req, payload, MAX_FORWARD_BODY_BYTES)
        .await
        .map_err(|e| match e {
            BodyError::TooLarge => error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                "the body is too large",
            ),
            BodyError::Read => error(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "the body could not be read",
            ),
        })
}

/// Host response headers passed through to the client unchanged.
const PASSED_HEADERS: &[&str] = &[RIM_HOST_RESPONSE, RIM_HOST_SIGNATURE, "retry-after"];

/// The host's answer as the client's response: status, body, `Content-Type`, the host-response
/// pair and `Retry-After`.
fn host_answer(answer: S2sResponse) -> HttpResponse {
    let status = StatusCode::from_u16(answer.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = HttpResponse::build(status);
    if !answer.content_type.is_empty() {
        response.content_type(answer.content_type.as_str());
    }
    response.insert_header((header::X_CONTENT_TYPE_OPTIONS, "nosniff"));
    response.insert_header((header::CONTENT_SECURITY_POLICY, SANDBOX_CSP));
    if !is_json(&answer.content_type) {
        response.insert_header((header::CONTENT_DISPOSITION, "attachment"));
    }
    for (name, value) in &answer.headers {
        if PASSED_HEADERS.contains(&name.as_str()) {
            response.insert_header((name.as_str(), value.as_str()));
        }
    }
    response.body(answer.body)
}

/// Whether `content_type` names JSON, with or without parameters.
fn is_json(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
}

/// Marks the response to a forward the host gave no verified answer to.
struct UpstreamUnavailable;

/// Whether a forwarded path could name another host path once a URL parser reads it.
///
/// `raw_path` is the request path as sent and `tail` the route's decoded tail. Refused: an
/// encoded slash or backslash (`%2F`, `%5C`, in any letter case) in either, and in `tail`
/// decoded once more, a backslash, a control character, or a `.` or `..` segment.
fn is_unsafe_tail(raw_path: &str, tail: &str) -> bool {
    let encoded_separator = |s: &str| {
        let lower = s.to_ascii_lowercase();
        lower.contains("%2f") || lower.contains("%5c")
    };
    if encoded_separator(raw_path) || encoded_separator(tail) {
        return true;
    }
    let decoded = percent_decode_str(tail).decode_utf8_lossy();
    decoded.chars().any(|c| c == '\\' || c.is_control())
        || decoded
            .split('/')
            .any(|segment| segment == "." || segment == "..")
}

/// Forward a local user's device-signed request to the host it names, and count it.
pub async fn handle_remote(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    payload: web::Payload,
    state: web::Data<S2sState>,
    authenticator: web::Data<Arc<dyn RequestAuthenticator>>,
) -> HttpResponse {
    let response = forward(req, path, payload, state, authenticator).await;
    let outcome = if response.extensions().contains::<UpstreamUnavailable>() {
        "upstream_unavailable"
    } else {
        crate::metrics::forward_outcome(response.status())
    };
    crate::metrics::forwarded("outgoing", outcome);
    response
}

/// Check a local user's device-signed request and forward it to the host it names.
async fn forward(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    payload: web::Payload,
    state: web::Data<S2sState>,
    authenticator: web::Data<Arc<dyn RequestAuthenticator>>,
) -> HttpResponse {
    if !state.settings.role.runs(Role::Relay) {
        return error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "this server is not a relay",
        );
    }
    if state.settings.origin.is_none() {
        return error(
            StatusCode::NOT_FOUND,
            "not_found",
            "server-to-server traffic is off on this server",
        );
    }
    let method = match req.method().as_str() {
        "GET" => S2sMethod::Get,
        "POST" => S2sMethod::Post,
        _ => return error(StatusCode::METHOD_NOT_ALLOWED, "bad_request", "GET or POST"),
    };
    if method == S2sMethod::Post
        && let Err(response) = require_csrf(&req)
    {
        return response;
    }
    let auth = match extract_auth(&req, authenticator.get_ref().as_ref()).await {
        Ok(auth) => auth,
        Err(e) => return map_service_error_to_response(e),
    };
    let Some(user_id) = auth.user_id else {
        return error(
            StatusCode::UNAUTHORIZED,
            "unauthenticated",
            "authentication required",
        );
    };
    let (host, tail) = path.into_inner();
    if is_unsafe_tail(req.uri().path(), &tail) {
        return error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "the path has a `.` or `..` segment, an encoded slash or backslash, or a control character",
        );
    }
    if !rim_s2s::is_server_id(&host) {
        return error(StatusCode::NOT_FOUND, "not_found", "not a server id");
    }
    if host == state.identity.current().server_id() {
        return error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "spaces this server hosts are used through /api",
        );
    }
    let (Some(request_header), Some(signature)) = (
        header(&req, RIM_DEVICE_REQUEST),
        header(&req, RIM_DEVICE_SIGNATURE),
    ) else {
        return error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Rim-Device-Request and Rim-Device-Signature are required",
        );
    };
    let Some(request_payload) = URL_SAFE_NO_PAD
        .decode(request_header.trim())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "Rim-Device-Request is not base64url",
        );
    };
    let named_host = match state
        .relay
        .check_outgoing_request(
            user_id,
            auth.device_key_id,
            request_payload,
            signature.clone(),
        )
        .await
    {
        Ok(named) => named,
        Err(e) => return map_service_error_to_response(e),
    };
    if named_host != host {
        return error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "the device request names another host",
        );
    }
    let body = match method {
        S2sMethod::Post => match read_body(&req, payload).await {
            Ok(body) => body,
            Err(response) => return response,
        },
        S2sMethod::Get => Vec::new(),
    };
    let forward_path = match req.uri().query() {
        Some(query) => format!("/forward/{tail}?{query}"),
        None => format!("/forward/{tail}"),
    };
    let answer = state
        .client
        .send(S2sRequest {
            peer_server_id: host.clone(),
            method,
            path: forward_path,
            body,
            content_type: header(&req, "content-type"),
            headers: vec![
                (RIM_DEVICE_REQUEST.to_string(), request_header),
                (RIM_DEVICE_SIGNATURE.to_string(), signature),
            ],
        })
        .await;
    match answer {
        Ok(answer) => host_answer(answer),
        Err(e) => {
            tracing::warn!(host_server_id = %host, error = %e, "a forwarded request found no verified answer");
            let mut response = HttpResponse::BadGateway().json(serde_json::json!({
                "error": "upstream_unavailable",
                "message": "the host could not be reached",
                "hostServerId": host,
            }));
            response.extensions_mut().insert(UpstreamUnavailable);
            response
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Mutex;

    use actix_web::{App, test};
    use chrono::{DateTime, Utc};
    use rim_core::key_identity::model::DeviceKeyId;
    use rim_core::user::model::UserId;
    use rim_infra_services::KeyedRateLimiter;
    use rim_s2s::{
        Endpoints, Limits, Registration, RegistrationMode, ServerDescriptor, ServerIdentity,
        ServerKeyState, ServerSigner, SharedIdentity, sign_descriptor, sign_inception, timestamp,
    };
    use rim_services::auth::dto::RequestAuthResultDto;
    use rim_services::auth::service::MockRequestAuthenticator;
    use rim_services::host::service::MockHostService;
    use rim_services::relay::service::MockRelayService;
    use rim_services::remote_workspace::service::{
        MockWorkspaceHostService, MockWorkspaceRelayService,
    };
    use rim_services::s2s::service::{MockPeerService, MockS2sClient, MockS2sNonceService};

    use super::*;
    use crate::config::ServerRole;
    use crate::s2s::S2sSettings;

    #[actix_web::test]
    async fn a_hosts_html_answer_is_a_download_that_nothing_renders() {
        let response = host_answer(S2sResponse {
            status: 200,
            content_type: "text/html".to_string(),
            body: b"<script>1</script>".to_vec(),
            key_seq: 0,
            headers: vec![],
        });
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        assert_eq!(header("content-disposition").as_deref(), Some("attachment"));
        assert_eq!(header("x-content-type-options").as_deref(), Some("nosniff"));
    }

    #[actix_web::test]
    async fn a_hosts_answer_keeps_its_retry_after_and_host_response_headers() {
        let response = host_answer(S2sResponse {
            status: 429,
            content_type: "application/json".to_string(),
            body: br#"{"error":"rate_limited"}"#.to_vec(),
            key_seq: 0,
            headers: vec![
                (RIM_HOST_RESPONSE.to_string(), "cGF5bG9hZA".to_string()),
                (RIM_HOST_SIGNATURE.to_string(), "ed25519:sig".to_string()),
                ("retry-after".to_string(), "17".to_string()),
                ("x-other".to_string(), "dropped".to_string()),
            ],
        });
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        assert_eq!(header("retry-after").as_deref(), Some("17"));
        assert_eq!(header(RIM_HOST_RESPONSE).as_deref(), Some("cGF5bG9hZA"));
        assert_eq!(header(RIM_HOST_SIGNATURE).as_deref(), Some("ed25519:sig"));
        assert_eq!(header("content-type").as_deref(), Some("application/json"));
        assert_eq!(header("x-other"), None);
        assert_eq!(header("x-content-type-options").as_deref(), Some("nosniff"));
        assert_eq!(
            header("content-security-policy").as_deref(),
            Some(SANDBOX_CSP)
        );
        assert_eq!(header("content-disposition"), None);
        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .expect("body");
        assert_eq!(&body[..], br#"{"error":"rate_limited"}"#);
    }

    /// The host the tests forward to; not this server.
    const HOST: &str = "rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c";

    /// A relay identity from a fixed seed.
    fn identity() -> SharedIdentity {
        let signer = ServerSigner::from_seed([3; 32]);
        let recovery = ServerSigner::from_seed([103; 32]);
        let incepted_at = DateTime::from_timestamp(1_767_225_600, 0).expect("2026-01-01");
        let inception = sign_inception(&signer, &recovery, incepted_at);
        let state = ServerKeyState::from_inception(&inception).expect("inception");
        let content = ServerDescriptor::new(
            state.server_id.clone(),
            0,
            1,
            vec![Role::Relay],
            Endpoints {
                graphql: "https://relay.example/api".into(),
                websocket: "wss://relay.example/api/ws".into(),
                s2s: Some("https://relay.example/s2s/v1".into()),
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
            None,
            timestamp(Utc::now()),
        );
        let descriptor = sign_descriptor(&signer, &content).expect("descriptor");
        SharedIdentity::new(ServerIdentity {
            signer,
            state,
            log: vec![inception],
            descriptor,
            descriptor_content: content,
        })
    }

    /// A relay whose user's device requests name [`HOST`], and whose client records each
    /// path it is asked to send and answers `200`.
    fn relay_state(sent: Arc<Mutex<Vec<String>>>) -> S2sState {
        let mut relay = MockRelayService::new();
        relay
            .expect_check_outgoing_request()
            .returning(|_, _, _, _| Box::pin(async { Ok(HOST.to_string()) }));
        let mut client = MockS2sClient::new();
        client.expect_send().returning(move |request| {
            sent.lock().expect("lock").push(request.path);
            Box::pin(async {
                Ok(S2sResponse {
                    status: 200,
                    content_type: "application/json".to_string(),
                    body: b"{}".to_vec(),
                    key_seq: 0,
                    headers: Vec::new(),
                })
            })
        });
        S2sState {
            identity: identity(),
            peers: Arc::new(MockPeerService::new()),
            nonces: Arc::new(MockS2sNonceService::new()),
            rate_limiter: Arc::new(KeyedRateLimiter::new(60, 60)),
            settings: Arc::new(S2sSettings {
                role: ServerRole::Relay,
                origin: Some("https://relay.example".to_string()),
                denied_peers: HashSet::new(),
                allowed_relays: HashSet::new(),
                max_body_bytes: 1024,
            }),
            client: Arc::new(client),
            relay: Arc::new(relay),
            host: Arc::new(MockHostService::new()),
            workspace_host: Arc::new(MockWorkspaceHostService::new()),
            workspace_relay: Arc::new(MockWorkspaceRelayService::new()),
        }
    }

    /// An authenticator that signs every request in as one user with one device.
    fn authenticator() -> Arc<dyn RequestAuthenticator> {
        let mut authenticator = MockRequestAuthenticator::new();
        authenticator
            .expect_authenticate_session_token()
            .returning(|_, _| {
                Box::pin(async {
                    Ok(RequestAuthResultDto {
                        user_id: Some(UserId::new()),
                        session_id: None,
                        device_key_id: Some(DeviceKeyId::new()),
                        expires_at: None,
                        secret_hash: None,
                    })
                })
            });
        Arc::new(authenticator)
    }

    /// `GET` `uri` with device headers through the relay route; the status and the paths the
    /// client was asked to send.
    async fn get(uri: &str) -> (StatusCode, Vec<String>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let app = test::init_service(
            App::new()
                .app_data(web::Data::new(relay_state(Arc::clone(&sent))))
                .app_data(web::Data::new(authenticator()))
                .route(
                    "/api/remote/{host_server_id}/{tail:.*}",
                    web::get().to(handle_remote),
                ),
        )
        .await;
        let request = test::TestRequest::get()
            .uri(uri)
            .insert_header((RIM_DEVICE_REQUEST, URL_SAFE_NO_PAD.encode("{}")))
            .insert_header((RIM_DEVICE_SIGNATURE, "ed25519:sig"))
            .to_request();
        let status = test::call_service(&app, request).await.status();
        let sent = sent.lock().expect("lock").clone();
        (status, sent)
    }

    #[actix_web::test]
    async fn a_tail_forwards_to_the_hosts_forward_path() {
        assert!(rim_s2s::is_server_id(HOST));
        let (status, sent) = get(&format!(
            "/api/remote/{HOST}/api/attachments/x/download?a=1"
        ))
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            sent,
            vec!["/forward/api/attachments/x/download?a=1".to_string()]
        );
    }

    #[actix_web::test]
    async fn a_tail_with_a_dot_segment_is_refused_before_anything_is_sent() {
        for tail in [
            "../outbox",
            "%2e%2e/outbox",
            "a/%2E%2E/%2e%2e/outbox",
            "./outbox",
            "api/..",
            "%2e%2e%2Foutbox",
            "..%2foutbox",
            "%252e%252e/outbox",
            "api%2Fx",
            "..%5Coutbox",
            "api%5Cx",
            ".%09./outbox",
        ] {
            let (status, sent) = get(&format!("/api/remote/{HOST}/{tail}?after=0")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{tail}");
            assert!(sent.is_empty(), "{tail}: {sent:?}");
        }
    }

    #[actix_web::test]
    async fn dots_inside_a_segment_are_not_dot_segments() {
        let (status, sent) = get(&format!("/api/remote/{HOST}/api/a..b/.x/x.")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(sent, vec!["/forward/api/a..b/.x/x.".to_string()]);
    }
}
