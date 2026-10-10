//! `GET|POST /api/remote/<hostServerId>/<path>`: a local user's request to a space hosted on
//! another server. The relay checks the session and the device request, then forwards the body
//! unchanged to the host's `/s2s/v1/forward/<path>` and returns the host's answer unchanged.

use std::sync::Arc;

use actix_web::http::StatusCode;
use actix_web::{HttpRequest, HttpResponse, web};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::StreamExt;
use rim_s2s::Role;
use rim_services::auth::service::RequestAuthenticator;
use rim_services::s2s::dto::{S2sMethod, S2sRequest, S2sResponse};

use crate::file_handlers::{map_service_error_to_response, require_csrf};
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

async fn read_body(mut payload: web::Payload) -> Result<Vec<u8>, HttpResponse> {
    let mut body = Vec::new();
    while let Some(chunk) = payload.next().await {
        let chunk = chunk.map_err(|_| {
            error(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "the body could not be read",
            )
        })?;
        if body.len() + chunk.len() > MAX_FORWARD_BODY_BYTES {
            return Err(error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                "the body is too large",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
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
    for (name, value) in &answer.headers {
        if PASSED_HEADERS.contains(&name.as_str()) {
            response.insert_header((name.as_str(), value.as_str()));
        }
    }
    response.body(answer.body)
}

/// Forward a local user's device-signed request to the host it names.
pub async fn handle_remote(
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
        S2sMethod::Post => match read_body(payload).await {
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
            HttpResponse::BadGateway().json(serde_json::json!({
                "error": "upstream_unavailable",
                "message": "the host could not be reached",
                "hostServerId": host,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .expect("body");
        assert_eq!(&body[..], br#"{"error":"rate_limited"}"#);
    }
}
