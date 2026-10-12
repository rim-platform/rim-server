//! `/s2s/v1/forward/<path>`: a device-signed request a relay forwarded, run as its user and
//! device with no session, answered with a host-signed response.

use std::sync::Arc;

use actix_multipart::Multipart;
use actix_web::http::StatusCode;
use actix_web::{HttpRequest, HttpResponse, web};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rim_api::AppSchema;
use rim_api::auth_context::{AuthActorCtx, RemoteIp};
use rim_api::auth_intent::AuthIntentCtx;
use rim_api::federation::{ForwardedRequest, refused_account_mutation};
use rim_core::server_feature::model::ServerFeature;
use rim_core::storage::StorageBackend;
use rim_s2s::host::sign_host_response;
use rim_services::feature::service::FeatureGate;
use rim_services::host::dto::{ForwardAuth, ForwardRefusal, ForwardRequest};
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::storage::service::StorageService;
use rim_services::workspace_plugin::service::PluginBundleService;
use serde_json::json;

use crate::s2s::S2sState;
use crate::s2s::guard::{Reply, Verified, internal};

/// `Rim-Host-Response`: the host-response payload, unpadded base64url.
pub(crate) const RIM_HOST_RESPONSE: &str = "rim-host-response";
/// `Rim-Host-Signature`: its signature.
pub(crate) const RIM_HOST_SIGNATURE: &str = "rim-host-signature";
/// `Rim-Device-Request`: the device-request payload, unpadded base64url.
pub(crate) const RIM_DEVICE_REQUEST: &str = "rim-device-request";
/// `Rim-Device-Signature`: its signature.
pub(crate) const RIM_DEVICE_SIGNATURE: &str = "rim-device-signature";

fn refusal(refusal: ForwardRefusal) -> Reply {
    match refusal {
        ForwardRefusal::BadRequest(message) => {
            Reply::error(StatusCode::BAD_REQUEST, "bad_request", &message)
        }
        ForwardRefusal::Forbidden(message) => {
            Reply::error(StatusCode::FORBIDDEN, "forbidden", &message)
        }
        ForwardRefusal::NotDelegated => Reply::error(
            StatusCode::FORBIDDEN,
            "not_delegated",
            "the relay may not carry this user's traffic",
        ),
        ForwardRefusal::Replay { highest } => Reply::json(
            StatusCode::CONFLICT,
            &json!({
                "error": "replay",
                "message": "the device request seq was refused",
                "highestSeq": highest.to_string(),
            }),
        ),
        ForwardRefusal::RateLimited {
            retry_after_seconds,
        } => {
            let mut reply = Reply::error(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "too many requests from this device",
            );
            reply.retry_after = Some(retry_after_seconds.max(1));
            reply
        }
        ForwardRefusal::Internal => internal("forwarded request check failed"),
    }
}

/// An HTTP response of a shared handler, as a reply to sign.
async fn reply_of(response: HttpResponse) -> Reply {
    let status = response.status();
    let content_type = response
        .headers()
        .get(actix_web::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    match actix_web::body::to_bytes(response.into_body()).await {
        Ok(bytes) => Reply {
            status,
            body: bytes.to_vec(),
            content_type,
            retry_after: None,
            headers: Vec::new(),
        },
        Err(_) => internal("could not read a forwarded response"),
    }
}

/// Run GraphQL as the forwarded user and device.
async fn graphql(
    req: &HttpRequest,
    auth: &ForwardAuth,
    peer_server_id: &str,
    body: &[u8],
) -> Reply {
    let Some(schema) = req.app_data::<web::Data<AppSchema>>() else {
        return internal("the schema is not configured");
    };
    let batch: async_graphql::BatchRequest = match serde_json::from_slice(body) {
        Ok(batch) => batch,
        Err(_) => {
            return Reply::error(StatusCode::BAD_REQUEST, "bad_request", "malformed GraphQL");
        }
    };
    if let Some(field) = batch.iter().find_map(|request| {
        refused_account_mutation(&request.query, request.operation_name.as_deref())
    }) {
        tracing::info!(field, user_id = %auth.user_id, "refused an account mutation for a remote member");
        return Reply::ok(json!({
            "errors": [{
                "message": "not available to remote members",
                "extensions": { "code": "FORBIDDEN" },
            }],
        }));
    }
    let batch = batch
        .data(
            AuthActorCtx::new(Some(auth.user_id), None)
                .with_device_key_id(Some(auth.device_key_id)),
        )
        .data(Arc::new(AuthIntentCtx::default()))
        .data(RemoteIp::new(format!(
            "relay:{peer_server_id}:{}",
            auth.principal_id
        )))
        .data(ForwardedRequest);
    let response = schema.execute_batch(batch).await;
    match serde_json::to_value(&response) {
        Ok(body) => Reply::ok(body),
        Err(e) => internal(e),
    }
}

/// The forwarded body as a multipart stream, under the request's `Content-Type`.
fn multipart(req: &HttpRequest, verified: &Verified) -> Multipart {
    let bytes = actix_web::web::Bytes::from(verified.body.clone());
    let stream =
        futures_util::stream::once(async move { Ok::<_, actix_web::error::PayloadError>(bytes) });
    Multipart::new(req.headers(), stream)
}

/// The feature a forwarded file route belongs to, when it belongs to one.
fn route_feature(method: &str, segments: &[&str]) -> Option<ServerFeature> {
    match (method, segments) {
        ("GET", ["api", "attachments", _, "download"]) | ("POST", ["api", "files", "upload"]) => {
            Some(ServerFeature::Attachments)
        }
        ("GET", ["api", "plugin-bundles", _, _]) => Some(ServerFeature::WorkspacePlugins),
        _ => None,
    }
}

/// Run the forwarded request's handler as its user and device.
async fn execute(
    req: &HttpRequest,
    verified: &Verified,
    auth: &ForwardAuth,
    method: &str,
    path: &str,
) -> Reply {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if let Some(feature) = route_feature(method, &segments) {
        let Some(features) = req.app_data::<web::Data<Arc<dyn FeatureGate>>>() else {
            return internal("features are not configured");
        };
        if let Err(resp) =
            crate::file_handlers::require_feature(features.get_ref().as_ref(), feature)
        {
            return reply_of(resp).await;
        }
    }
    match (method, segments.as_slice()) {
        ("POST", ["api"]) => graphql(req, auth, &verified.peer.server_id, &verified.body).await,
        ("GET", ["api", "attachments", id, "download"]) => {
            let (Some(attachments), Some(storage)) = (
                req.app_data::<web::Data<Arc<dyn MessageAttachmentService>>>(),
                req.app_data::<web::Data<Arc<dyn StorageBackend>>>(),
            ) else {
                return internal("attachments are not configured");
            };
            reply_of(
                crate::file_handlers::attachment_download_as(
                    auth.user_id,
                    id,
                    attachments.get_ref().as_ref(),
                    storage.get_ref().as_ref(),
                )
                .await,
            )
            .await
        }
        ("POST", ["api", "files", "upload"]) => {
            let Some(attachments) = req.app_data::<web::Data<Arc<dyn MessageAttachmentService>>>()
            else {
                return internal("attachments are not configured");
            };
            reply_of(
                crate::file_handlers::upload_as(
                    auth.user_id,
                    multipart(req, verified),
                    attachments.get_ref().as_ref(),
                )
                .await,
            )
            .await
        }
        ("POST", ["api", "files", "avatar"]) => {
            let Some(storage) = req.app_data::<web::Data<Arc<dyn StorageService>>>() else {
                return internal("storage is not configured");
            };
            reply_of(
                crate::file_handlers::avatar_upload_as(
                    auth.user_id,
                    multipart(req, verified),
                    storage.get_ref().as_ref(),
                )
                .await,
            )
            .await
        }
        ("GET", ["api", "files", file_path @ ..]) if !file_path.is_empty() => {
            let Some(storage) = req.app_data::<web::Data<Arc<dyn StorageBackend>>>() else {
                return internal("storage is not configured");
            };
            reply_of(
                crate::file_handlers::file_download_as(
                    &file_path.join("/"),
                    storage.get_ref().as_ref(),
                )
                .await,
            )
            .await
        }
        ("GET", ["api", "plugin-bundles", server_id, sha256]) => {
            let Some(bundles) = req.app_data::<web::Data<Arc<dyn PluginBundleService>>>() else {
                return internal("plugin bundles are not configured");
            };
            reply_of(
                crate::plugin_bundle_handlers::bundle_download_as(
                    auth.user_id,
                    server_id,
                    (*sha256).to_string(),
                    bundles.get_ref().as_ref(),
                )
                .await,
            )
            .await
        }
        _ => Reply::error(StatusCode::NOT_FOUND, "not_found", "no such forwarded path"),
    }
}

/// Add `Rim-Host-Response` and `Rim-Host-Signature` over the reply's status and body.
fn with_host_response(state: &S2sState, mut reply: Reply, request_payload: &str) -> Reply {
    let identity = state.identity.current();
    let statement = sign_host_response(
        &identity.signer,
        identity.server_id(),
        identity.state.seq,
        request_payload,
        reply.status.as_u16(),
        &reply.body,
    );
    reply.headers.push((
        RIM_HOST_RESPONSE.to_string(),
        URL_SAFE_NO_PAD.encode(statement.payload.as_bytes()),
    ));
    reply
        .headers
        .push((RIM_HOST_SIGNATURE.to_string(), statement.signature));
    reply
}

/// Check and run a forwarded request.
pub(crate) async fn run(state: &S2sState, req: &HttpRequest, verified: &Verified) -> Reply {
    let tail = req
        .path()
        .strip_prefix("/s2s/v1/forward")
        .unwrap_or_default()
        .to_string();
    let path = match req.uri().query() {
        Some(query) => format!("{tail}?{query}"),
        None => tail.clone(),
    };
    let method = req.method().as_str().to_string();
    let header = |name: &str| {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let outcome = state
        .host
        .authenticate_forward(ForwardRequest {
            peer_server_id: verified.peer.server_id.clone(),
            peer_origin: verified.peer.origin.clone(),
            device_request: header(RIM_DEVICE_REQUEST),
            device_signature: header(RIM_DEVICE_SIGNATURE),
            method: method.clone(),
            path,
            body: verified.body.clone(),
        })
        .await;
    let reply = match outcome.result {
        Err(refused) => refusal(refused),
        Ok(auth) => execute(req, verified, &auth, &method, &tail).await,
    };
    crate::metrics::forwarded("incoming", crate::metrics::forward_outcome(reply.status));
    match outcome.request_payload {
        Some(payload) => with_host_response(state, reply, &payload),
        None => reply,
    }
}
