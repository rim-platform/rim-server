//! Every request under `/s2s/v1/` passes the same checks, in order: body size, the signature
//! profile and clock window, the peer (denied, allow-list, pinned or pinnable, forked), the key
//! for `keyid`, the body digest and the signature, the nonce, the per-peer rate limit, and the
//! role. Every answer, refusals included, is signed and bound to the request's nonce.

use actix_web::http::StatusCode;
use actix_web::http::header::{HeaderName, HeaderValue};
use actix_web::{HttpRequest, HttpResponse, web};
use chrono::Utc;
use futures_util::StreamExt;
use rim_identity::PublicKey;
use rim_s2s::http_sig::{
    IncomingRequest, NONCE_RETENTION_SECS, OutgoingResponse, PendingRequest, RIM_ORIGIN,
    RIM_REQUEST_NONCE, RequestBinding, parse_request, request_nonce, sign_response,
};
use rim_services::ServiceError;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::relay::dto::{DeliverRequest, IdentityDocument, PeerClaimRequest};
use rim_services::remote_workspace::dto::{
    CancelRequest, JoinChallengeRequest, JoinRequest, PresenceRequest, RevocationAnswer,
    RevocationRequest, SubscriptionsAnswer, SubscriptionsRequest,
};
use rim_services::s2s::dto::{PeerKeysDto, PeerLookup};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::s2s::S2sState;

/// A request body as `T`; `400 bad_request` naming `what` when it is not one.
fn body_as<T: DeserializeOwned>(body: &[u8], what: &str) -> Result<T, Reply> {
    serde_json::from_slice(body).map_err(|_| {
        Reply::error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            &format!("malformed {what}"),
        )
    })
}

/// A service answer as a `200` reply of its JSON, or the refusal it maps to.
fn answer<T: serde::Serialize>(result: Result<T, ServiceError>) -> Reply {
    match result {
        Ok(value) => Reply::ok(serde_json::to_value(value).unwrap_or_default()),
        Err(e) => Reply::from_service_error(e),
    }
}

/// `GET /outbox` parameters.
#[derive(serde::Deserialize)]
struct OutboxQuery {
    after: Option<i64>,
    limit: Option<i64>,
}
use crate::s2s::roles::{Endpoint, serves};

/// Prefix of every server-to-server path.
const S2S_PREFIX: &str = "/s2s/v1";

/// Body cap on the forward path.
const FORWARD_MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

const JSON: &str = "application/json";

/// What a response is bound to: the request's method, target URI, digest and nonce.
struct Binding {
    method: String,
    target_uri: String,
    request_digest: Option<String>,
    nonce: String,
}

/// An answer before it is signed.
pub(crate) struct Reply {
    pub(crate) status: StatusCode,
    pub(crate) body: Vec<u8>,
    pub(crate) content_type: String,
    pub(crate) retry_after: Option<u64>,
    /// Further headers, outside the signature.
    pub(crate) headers: Vec<(String, String)>,
}

impl Reply {
    pub(crate) fn json(status: StatusCode, body: &Value) -> Self {
        Self {
            status,
            body: serde_json::to_vec(body).unwrap_or_default(),
            content_type: JSON.to_string(),
            retry_after: None,
            headers: Vec::new(),
        }
    }

    pub(crate) fn ok(body: Value) -> Self {
        Self::json(StatusCode::OK, &body)
    }

    pub(crate) fn error(status: StatusCode, code: &str, message: &str) -> Self {
        Self::json(status, &json!({ "error": code, "message": message }))
    }

    /// A service refusal as the S2S error it maps to.
    pub(crate) fn from_service_error(err: ServiceError) -> Self {
        match err {
            ServiceError::NotFound => Self::error(StatusCode::NOT_FOUND, "not_found", "not found"),
            ServiceError::Validation(message) => {
                Self::error(StatusCode::BAD_REQUEST, "bad_request", &message)
            }
            ServiceError::Forbidden(message) => {
                Self::error(StatusCode::FORBIDDEN, "forbidden", &message)
            }
            ServiceError::PermissionDenied => {
                Self::error(StatusCode::FORBIDDEN, "forbidden", "refused")
            }
            ServiceError::Conflict(message) => {
                Self::error(StatusCode::CONFLICT, "conflict", &message)
            }
            ServiceError::RateLimited {
                retry_after_seconds,
            } => {
                let mut reply = Self::error(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    "too many requests",
                );
                reply.retry_after = Some(retry_after_seconds.max(1));
                reply
            }
            other => internal(other),
        }
    }
}

/// A request whose signature, key, digest, nonce and rate checks all passed.
pub(crate) struct Verified {
    pub(crate) peer: PeerKeysDto,
    #[allow(dead_code)]
    pub(crate) pending: PendingRequest,
    pub(crate) body: Vec<u8>,
}

fn joined_header(req: &HttpRequest, name: &str) -> Option<String> {
    let values: Vec<&str> = req
        .headers()
        .get_all(name)
        .filter_map(|v| v.to_str().ok())
        .map(str::trim)
        .collect();
    (!values.is_empty()).then(|| values.join(", "))
}

enum BodyError {
    TooLarge,
    Read,
}

async fn read_body(
    req: &HttpRequest,
    mut payload: web::Payload,
    limit: usize,
) -> Result<Vec<u8>, BodyError> {
    let declared = req
        .headers()
        .get(actix_web::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|len| len > limit) {
        return Err(BodyError::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = payload.next().await {
        let chunk = chunk.map_err(|_| BodyError::Read)?;
        if body.len() + chunk.len() > limit {
            return Err(BodyError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Sign `reply` with this server's current key, bound to the request.
fn signed(state: &S2sState, binding: &Binding, reply: Reply) -> HttpResponse {
    let body = reply.body;
    let identity = state.identity.current();
    let key_id = identity.key_id();
    let headers = sign_response(
        &identity.signer,
        &OutgoingResponse {
            status: reply.status.as_u16(),
            body: &body,
            content_type: &reply.content_type,
            request: RequestBinding {
                method: &binding.method,
                target_uri: &binding.target_uri,
                content_digest: binding.request_digest.as_deref(),
                nonce: &binding.nonce,
            },
            created: Utc::now().timestamp(),
            key_id: &key_id,
        },
    );
    let mut response = HttpResponse::build(reply.status);
    response.content_type(reply.content_type.as_str());
    for (name, value) in &reply.headers {
        response.insert_header((name.as_str(), value.as_str()));
    }
    if let Ok(nonce) = HeaderValue::from_str(&binding.nonce) {
        response.insert_header((HeaderName::from_static(RIM_REQUEST_NONCE), nonce));
    }
    if let Some(headers) = headers {
        if let Some(digest) = headers.content_digest {
            response.insert_header(("content-digest", digest));
        }
        response.insert_header(("signature-input", headers.signature_input));
        response.insert_header(("signature", headers.signature));
    }
    if let Some(seconds) = reply.retry_after {
        response.insert_header(("retry-after", seconds.to_string()));
    }
    response.body(body)
}

fn refuse_signature(peer: Option<&str>, reason: &str) -> Reply {
    tracing::warn!(
        peer_server_id = peer.unwrap_or("-"),
        reason,
        "refused a server-to-server signature"
    );
    Reply::error(
        StatusCode::UNAUTHORIZED,
        "bad_signature",
        "the request signature was refused",
    )
}

pub(crate) fn internal(err: impl std::fmt::Display) -> Reply {
    tracing::error!(error = %err, "server-to-server request failed");
    Reply::error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "internal error",
    )
}

/// Run every check on a request; `Err` is the refusal to sign and send.
async fn verify(
    state: &S2sState,
    req: &HttpRequest,
    payload: web::Payload,
    binding: &Binding,
    endpoint: Option<Endpoint>,
) -> Result<Verified, Reply> {
    let path = req.path().strip_prefix(S2S_PREFIX).unwrap_or_default();
    let limit = if path.starts_with("/forward/") {
        FORWARD_MAX_BODY_BYTES
    } else {
        state.settings.max_body_bytes
    };
    let body = read_body(req, payload, limit).await.map_err(|e| match e {
        BodyError::TooLarge => Reply::error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "the request body is too large",
        ),
        BodyError::Read => Reply::error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "the request body could not be read",
        ),
    })?;

    let content_digest = joined_header(req, "content-digest");
    let content_type = joined_header(req, "content-type");
    let rim_origin = joined_header(req, RIM_ORIGIN);
    let signature_input = joined_header(req, "signature-input");
    let signature = joined_header(req, "signature");
    let pending = parse_request(
        &IncomingRequest {
            method: &binding.method,
            target_uri: &binding.target_uri,
            content_digest: content_digest.as_deref(),
            content_type: content_type.as_deref(),
            rim_origin: rim_origin.as_deref(),
            signature_input: signature_input.as_deref(),
            signature: signature.as_deref(),
        },
        Utc::now().timestamp(),
    )
    .map_err(|r| refuse_signature(None, r.as_str()))?;

    let peer_id = pending.key_id.server_id.clone();
    let not_allowed = || {
        tracing::warn!(peer_server_id = %peer_id, "refused a peer that is not allowed");
        Reply::error(
            StatusCode::FORBIDDEN,
            "peer_not_allowed",
            "this server does not accept requests from that server",
        )
    };
    if state.settings.denied_peers.contains(&peer_id) {
        return Err(not_allowed());
    }
    if endpoint.is_some_and(Endpoint::host_allow_list_applies)
        && !state.settings.allowed_relays.is_empty()
        && !state.settings.allowed_relays.contains(&peer_id)
    {
        return Err(not_allowed());
    }

    let keys = match state
        .peers
        .resolve(
            &peer_id,
            Some(&pending.rim_origin),
            Some(pending.key_id.seq),
        )
        .await
    {
        Ok(PeerLookup::Ready(keys)) => keys,
        Ok(PeerLookup::Unknown) => {
            return Err(Reply::error(
                StatusCode::FORBIDDEN,
                "unknown_peer",
                "the signing server could not be pinned from its origin",
            ));
        }
        Ok(PeerLookup::Forked) => {
            return Err(Reply::error(
                StatusCode::FORBIDDEN,
                "peer_forked",
                "the signing server's key log forked",
            ));
        }
        Ok(PeerLookup::Blocked) => return Err(not_allowed()),
        Err(e) => return Err(internal(e)),
    };
    let key_text = keys
        .key_for_seq(pending.key_id.seq)
        .ok_or_else(|| refuse_signature(Some(&peer_id), "key_not_accepted"))?;
    let key = PublicKey::parse(key_text, "peer key").map_err(internal)?;
    pending
        .check_digest(&body)
        .map_err(|r| refuse_signature(Some(&peer_id), r.as_str()))?;
    pending
        .verify(&key)
        .map_err(|r| refuse_signature(Some(&peer_id), r.as_str()))?;

    let expires_at = chrono::DateTime::from_timestamp(pending.created + NONCE_RETENTION_SECS, 0)
        .ok_or_else(|| refuse_signature(Some(&peer_id), "clock"))?;
    let fresh = state
        .nonces
        .accept(&peer_id, &pending.nonce, expires_at)
        .await
        .map_err(internal)?;
    if !fresh {
        tracing::warn!(peer_server_id = %peer_id, "refused a replayed server-to-server request");
        return Err(Reply::error(
            StatusCode::CONFLICT,
            "replay",
            "this request was seen before",
        ));
    }

    if let RateLimitDecision::Limited {
        retry_after_seconds,
    } = state.rate_limiter.check_and_record(&peer_id, Utc::now())
    {
        tracing::warn!(peer_server_id = %peer_id, "rate-limited a peer's server-to-server requests");
        let mut reply = Reply::error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many requests from this server",
        );
        reply.retry_after = Some(retry_after_seconds.max(1));
        return Err(reply);
    }
    Ok(Verified {
        peer: keys,
        pending,
        body,
    })
}

async fn dispatch(
    state: &S2sState,
    req: &HttpRequest,
    endpoint: Endpoint,
    verified: &Verified,
) -> Reply {
    let peer = verified.peer.server_id.clone();
    let parse = |body: &[u8]| -> Result<Value, Reply> {
        serde_json::from_slice(body)
            .map_err(|_| Reply::error(StatusCode::BAD_REQUEST, "bad_request", "malformed JSON"))
    };
    match endpoint {
        Endpoint::Principal => {
            let principal_id = req
                .path()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            match state.relay.local_identity(principal_id).await {
                Ok(Some(document)) => Reply::ok(serde_json::to_value(document).unwrap_or_default()),
                Ok(None) => Reply::error(StatusCode::NOT_FOUND, "not_found", "unknown principal"),
                Err(e) => Reply::from_service_error(e),
            }
        }
        Endpoint::Identity => {
            let document = match parse(&verified.body).and_then(|v| {
                serde_json::from_value::<IdentityDocument>(v).map_err(|_| {
                    Reply::error(StatusCode::BAD_REQUEST, "bad_request", "malformed identity")
                })
            }) {
                Ok(document) => document,
                Err(reply) => return reply,
            };
            match state.host.apply_identity(peer, document).await {
                Ok(applied) => Reply::ok(json!({ "applied": applied })),
                Err(e) => Reply::from_service_error(e),
            }
        }
        Endpoint::KeyPackageClaim => {
            let request = match parse(&verified.body).and_then(|v| {
                serde_json::from_value::<PeerClaimRequest>(v).map_err(|_| {
                    Reply::error(StatusCode::BAD_REQUEST, "bad_request", "malformed claim")
                })
            }) {
                Ok(request) => request,
                Err(reply) => return reply,
            };
            match state.relay.claim_for_peer(peer, request).await {
                Ok(answer) => Reply::ok(serde_json::to_value(answer).unwrap_or_default()),
                Err(e) => Reply::from_service_error(e),
            }
        }
        Endpoint::Deliver => {
            let request = match parse(&verified.body).and_then(|v| {
                serde_json::from_value::<DeliverRequest>(v).map_err(|_| {
                    Reply::error(StatusCode::BAD_REQUEST, "bad_request", "malformed delivery")
                })
            }) {
                Ok(request) => request,
                Err(reply) => return reply,
            };
            match state.relay.deliver(peer, request.items).await {
                Ok(answer) => Reply::ok(serde_json::to_value(answer).unwrap_or_default()),
                Err(e) => Reply::from_service_error(e),
            }
        }
        Endpoint::Outbox => {
            let query = web::Query::<OutboxQuery>::from_query(req.query_string()).ok();
            let (after, limit) = query
                .map(|q| (q.after.unwrap_or(0), q.limit.unwrap_or(256)))
                .unwrap_or((0, 256));
            match state.host.outbox_page(peer, after, limit).await {
                Ok(page) => Reply::ok(serde_json::to_value(page).unwrap_or_default()),
                Err(e) => Reply::from_service_error(e),
            }
        }
        Endpoint::Forward => crate::s2s::forward::run(state, req, verified).await,
        Endpoint::JoinChallenge => {
            match body_as::<JoinChallengeRequest>(&verified.body, "join challenge request") {
                Ok(request) => answer(
                    state
                        .workspace_host
                        .join_challenge(peer, request.invite_token)
                        .await,
                ),
                Err(reply) => reply,
            }
        }
        Endpoint::Join => match body_as::<JoinRequest>(&verified.body, "join") {
            Ok(request) => answer(state.workspace_host.join(peer, request).await),
            Err(reply) => reply,
        },
        Endpoint::Subscriptions => {
            match body_as::<SubscriptionsRequest>(&verified.body, "subscriptions") {
                Ok(request) => answer(
                    state
                        .workspace_host
                        .subscribe(peer, request.delegations)
                        .await
                        .map(|subscriptions| SubscriptionsAnswer { subscriptions }),
                ),
                Err(reply) => reply,
            }
        }
        Endpoint::SubscriptionsCancel => {
            match body_as::<CancelRequest>(&verified.body, "cancellation") {
                Ok(request) => answer(
                    state
                        .workspace_host
                        .cancel(peer, request.subscriptions)
                        .await
                        .map(|cancelled| json!({ "cancelled": cancelled })),
                ),
                Err(reply) => reply,
            }
        }
        Endpoint::Presence => match body_as::<PresenceRequest>(&verified.body, "presence") {
            Ok(request) => answer(
                state
                    .workspace_host
                    .presence(peer, request.updates)
                    .await
                    .map(|applied| json!({ "applied": applied })),
            ),
            Err(reply) => reply,
        },
        Endpoint::Revocations => match body_as::<RevocationRequest>(&verified.body, "revocation") {
            Ok(request) => answer(
                state
                    .workspace_host
                    .apply_revocation(Some(peer), Some(request.principal_id), request.revocation)
                    .await
                    .map(|ack| RevocationAnswer { ack }),
            ),
            Err(reply) => reply,
        },
        Endpoint::Ping => {
            let identity = state.identity.current();
            let roles: Vec<&str> = state
                .settings
                .role
                .roles()
                .into_iter()
                .map(rim_s2s::Role::as_str)
                .collect();
            Reply::ok(json!({ "serverId": identity.server_id(), "roles": roles }))
        }
    }
}

/// Handle any request under `/s2s/v1/`.
pub(crate) async fn handle(
    req: HttpRequest,
    payload: web::Payload,
    state: web::Data<S2sState>,
) -> HttpResponse {
    let Some(origin) = state.settings.origin.as_deref() else {
        return HttpResponse::NotFound().json(json!({
            "error": "not_found",
            "message": "server-to-server traffic is off on this server",
        }));
    };
    let method = req.method().as_str().to_string();
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let binding = Binding {
        target_uri: format!("{origin}{path_and_query}"),
        request_digest: (method == "POST")
            .then(|| joined_header(&req, "content-digest"))
            .flatten(),
        nonce: joined_header(&req, "signature-input")
            .as_deref()
            .and_then(request_nonce)
            .unwrap_or_default(),
        method,
    };
    let path = req.path().strip_prefix(S2S_PREFIX).unwrap_or_default();
    let endpoint = Endpoint::route(&binding.method, path);

    let verified = match verify(&state, &req, payload, &binding, endpoint).await {
        Ok(verified) => verified,
        Err(reply) => return signed(&state, &binding, reply),
    };
    let reply = match endpoint {
        None => Reply::error(StatusCode::NOT_FOUND, "not_found", "no such endpoint"),
        Some(endpoint) if !serves(state.settings.role, endpoint.role()) => Reply::error(
            StatusCode::SERVICE_UNAVAILABLE,
            "role_disabled",
            "this server does not run the role that endpoint belongs to",
        ),
        Some(endpoint) => dispatch(&state, &req, endpoint, &verified).await,
    };
    tracing::debug!(
        peer_server_id = %verified.peer.server_id,
        status = reply.status.as_u16(),
        "answered a server-to-server request"
    );
    signed(&state, &binding, reply)
}
