//! The plugin bundle endpoints: a workspace's Owners and Moderators upload web plugin bundles,
//! and its members download them. The body is the bundle file itself, not multipart, and is
//! hashed as it is read.

use std::sync::Arc;

use actix_web::http::header;
use actix_web::{HttpRequest, HttpResponse, web};
use rim_core::server::model::ServerId;
use rim_core::server_feature::model::ServerFeature;
use rim_services::feature::service::FeatureGate;
use rim_services::workspace_plugin::command::{DownloadPluginBundleQuery, UploadPluginBundleCmd};
use rim_services::workspace_plugin::service::PluginBundleService;
use rim_services::workspace_plugin::{MAX_PLUGIN_BUNDLE_BYTES, PLUGIN_BUNDLE_MEDIA_TYPE};
use rim_services::{Actor, NOT_IN_DIRECT_SPACE, ServiceError};
use sha2::{Digest, Sha256};

use crate::body::{BodyError, read_bounded};
use crate::file_handlers::{
    map_service_error_to_response, require_auth, require_csrf, require_download_auth,
    require_feature,
};

/// Bundles are named by their hash, so a URL's bytes never change.
const BUNDLE_CACHE_CONTROL: &str = "private, max-age=31536000, immutable";

fn parse_server_id(raw: &str) -> Option<ServerId> {
    uuid::Uuid::parse_str(raw).ok().map(ServerId::from)
}

fn is_bundle_media_type(req: &HttpRequest) -> bool {
    req.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|essence| {
            essence
                .trim()
                .eq_ignore_ascii_case(PLUGIN_BUNDLE_MEDIA_TYPE)
        })
}

fn too_large() -> HttpResponse {
    HttpResponse::PayloadTooLarge().json(serde_json::json!({
        "error": format!("a bundle is at most {MAX_PLUGIN_BUNDLE_BYTES} bytes")
    }))
}

/// Read the whole body and its lowercase hex SHA-256. `Err` with the response for a body over
/// the limit or one that could not be read.
async fn read_body(
    req: &HttpRequest,
    payload: web::Payload,
) -> Result<(Vec<u8>, String), HttpResponse> {
    let bytes = read_bounded(req, payload, MAX_PLUGIN_BUNDLE_BYTES)
        .await
        .map_err(|e| match e {
            BodyError::TooLarge => too_large(),
            BodyError::Read => {
                HttpResponse::BadRequest().json(serde_json::json!({"error": "read error"}))
            }
        })?;
    let sha256 = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((bytes, sha256))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchBundleRequest {
    url: String,
    sha256: String,
}

/// POST /api/plugin-bundles/fetch: an authenticated, pinned and validated HTTPS bundle.
pub async fn handle_plugin_bundle_fetch(
    req: HttpRequest,
    body: web::Json<FetchBundleRequest>,
    service: web::Data<Arc<dyn PluginBundleService>>,
    features: web::Data<Arc<dyn FeatureGate>>,
) -> HttpResponse {
    if let Err(response) =
        require_feature(features.get_ref().as_ref(), ServerFeature::PluginLinkFetch)
    {
        return response;
    }
    let user_id = match require_auth(&req).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    if let Err(response) = require_csrf(&req) {
        return response;
    }
    match service
        .fetch(
            rim_services::workspace_plugin::command::FetchPluginBundleCmd {
                actor: Actor::new(user_id),
                url: body.url.clone(),
                sha256: body.sha256.clone(),
            },
        )
        .await
    {
        Ok(bytes) => HttpResponse::Ok()
            .content_type(PLUGIN_BUNDLE_MEDIA_TYPE)
            .insert_header((header::CACHE_CONTROL, "no-store"))
            .insert_header((header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
            .insert_header((header::CONTENT_DISPOSITION, "attachment"))
            .body(bytes),
        Err(ServiceError::Validation(reason)) => {
            HttpResponse::UnprocessableEntity().json(serde_json::json!({"error": reason}))
        }
        Err(error) => map_service_error_to_response(error),
    }
}

/// POST /api/plugin-bundles/{server_id}
pub async fn handle_plugin_bundle_upload(
    req: HttpRequest,
    path: web::Path<String>,
    payload: web::Payload,
    service: web::Data<Arc<dyn PluginBundleService>>,
    features: web::Data<Arc<dyn FeatureGate>>,
) -> HttpResponse {
    if let Err(resp) = require_feature(features.get_ref().as_ref(), ServerFeature::WorkspacePlugins)
    {
        return resp;
    }
    let user_id = match require_auth(&req).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_csrf(&req) {
        return resp;
    }
    let Some(server_id) = parse_server_id(&path.into_inner()) else {
        return HttpResponse::Forbidden().json(serde_json::json!({"error": "access denied"}));
    };
    if !is_bundle_media_type(&req) {
        return HttpResponse::UnsupportedMediaType().json(serde_json::json!({
            "error": format!("a bundle is sent as {PLUGIN_BUNDLE_MEDIA_TYPE}")
        }));
    }
    let (bytes, sha256) = match read_body(&req, payload).await {
        Ok(body) => body,
        Err(resp) => return resp,
    };

    let result = service
        .upload(UploadPluginBundleCmd {
            actor: Actor::new(user_id),
            server_id,
            bytes,
            sha256,
        })
        .await;
    match result {
        Ok(uploaded) => {
            let body = serde_json::json!({
                "sha256": uploaded.sha256,
                "size": uploaded.size,
                "url": uploaded.url,
            });
            if uploaded.created {
                HttpResponse::Created().json(body)
            } else {
                HttpResponse::Ok().json(body)
            }
        }
        Err(ServiceError::Validation(reason)) if reason == NOT_IN_DIRECT_SPACE => {
            HttpResponse::BadRequest().json(serde_json::json!({"error": reason}))
        }
        Err(ServiceError::Validation(reason)) => {
            HttpResponse::UnprocessableEntity().json(serde_json::json!({"error": reason}))
        }
        Err(e) => map_service_error_to_response(e),
    }
}

/// GET /api/plugin-bundles/{server_id}/{sha256}
pub async fn handle_plugin_bundle_download(
    req: HttpRequest,
    path: web::Path<(String, String)>,
    service: web::Data<Arc<dyn PluginBundleService>>,
    features: web::Data<Arc<dyn FeatureGate>>,
) -> HttpResponse {
    if let Err(resp) = require_feature(features.get_ref().as_ref(), ServerFeature::WorkspacePlugins)
    {
        return resp;
    }
    let user_id = match require_download_auth(&req).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let (server_id, sha256) = path.into_inner();
    bundle_download_as(user_id, &server_id, sha256, service.get_ref().as_ref()).await
}

/// Serve a plugin bundle to `user_id` if they may read it: the session path above and
/// forwarded requests from another relay share it.
pub(crate) async fn bundle_download_as(
    user_id: rim_core::user::model::UserId,
    server_id: &str,
    sha256: String,
    service: &dyn PluginBundleService,
) -> HttpResponse {
    let Some(server_id) = parse_server_id(server_id) else {
        return map_service_error_to_response(ServiceError::NotFound);
    };
    match service
        .download(DownloadPluginBundleQuery {
            actor: Actor::new(user_id),
            server_id,
            sha256,
        })
        .await
    {
        Ok(bytes) => HttpResponse::Ok()
            .content_type(PLUGIN_BUNDLE_MEDIA_TYPE)
            .insert_header((header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
            .insert_header((header::CONTENT_DISPOSITION, "attachment"))
            .insert_header((header::CACHE_CONTROL, BUNDLE_CACHE_CONTROL))
            .body(bytes),
        Err(e) => map_service_error_to_response(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::App;
    use actix_web::cookie::Cookie;
    use actix_web::test as actix_test;
    use rim_core::auth::model::SessionId;
    use rim_core::user::model::UserId;
    use rim_services::auth::dto::RequestAuthResultDto;
    use rim_services::auth::service::{MockRequestAuthenticator, RequestAuthenticator};
    use rim_services::workspace_plugin::dto::UploadedPluginBundleDto;
    use rim_services::workspace_plugin::service::MockPluginBundleService;

    use crate::graphql_support::{COOKIE_CSRF, COOKIE_SESSION};

    const SESSION: &str = "rim_sess_test";
    const BUNDLE: &[u8] =
        br#"{"format":"rim-plugin-bundle","v":1,"files":[{"path":"rim-plugin.json","text":"{}"}]}"#;

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// An authenticator that knows one session, of `user_id`.
    fn authenticator(user_id: UserId) -> MockRequestAuthenticator {
        let mut auth = MockRequestAuthenticator::new();
        auth.expect_authenticate_session_token()
            .returning(move |token, _| {
                let known = token == Some(SESSION);
                Box::pin(async move {
                    Ok(if known {
                        RequestAuthResultDto {
                            user_id: Some(user_id),
                            session_id: Some(SessionId::new()),
                            device_key_id: None,
                            expires_at: None,
                            secret_hash: None,
                        }
                    } else {
                        RequestAuthResultDto::default()
                    })
                })
            });
        auth
    }

    macro_rules! test_app {
        ($auth:expr, $service:expr) => {
            test_app!(
                $auth,
                $service,
                rim_infra_services::feature_gate::InstanceFeatureGate::all_enabled()
            )
        };
        ($auth:expr, $service:expr, $gate:expr) => {
            actix_test::init_service(
                App::new()
                    .service(
                        web::resource("/api/plugin-bundles/fetch")
                            .app_data(web::JsonConfig::default().limit(8192))
                            .route(web::post().to(handle_plugin_bundle_fetch)),
                    )
                    .service(
                        web::resource("/api/plugin-bundles/{server_id}")
                            .route(web::post().to(handle_plugin_bundle_upload)),
                    )
                    .service(
                        web::resource("/api/plugin-bundles/{server_id}/{sha256}")
                            .route(web::get().to(handle_plugin_bundle_download)),
                    )
                    .app_data(web::Data::new(
                        Arc::new($auth) as Arc<dyn RequestAuthenticator>
                    ))
                    .app_data(web::Data::new(
                        Arc::new($service) as Arc<dyn PluginBundleService>
                    ))
                    .app_data(web::Data::new(Arc::new($gate) as Arc<dyn FeatureGate>)),
            )
            .await
        };
    }

    /// A request with the session cookie and, when `csrf`, the matching CSRF cookie and header.
    fn signed_in(req: actix_test::TestRequest, csrf: bool) -> actix_test::TestRequest {
        let req = req.cookie(Cookie::new(COOKIE_SESSION, SESSION));
        if !csrf {
            return req;
        }
        let token = rim_auth::csrf::generate_csrf_token(SESSION);
        req.cookie(Cookie::new(COOKIE_CSRF, token.clone()))
            .insert_header(("x-csrf-token", token))
    }

    fn upload_request(server_id: ServerId, body: impl Into<web::Bytes>) -> actix_test::TestRequest {
        actix_test::TestRequest::post()
            .uri(&format!("/api/plugin-bundles/{server_id}"))
            .insert_header((header::CONTENT_TYPE, PLUGIN_BUNDLE_MEDIA_TYPE))
            .set_payload(body)
    }

    fn uploaded(server_id: ServerId, created: bool) -> UploadedPluginBundleDto {
        let sha256 = sha256_hex(BUNDLE);
        UploadedPluginBundleDto {
            url: format!("/api/plugin-bundles/{server_id}/{sha256}"),
            sha256,
            size: BUNDLE.len() as u64,
            created,
        }
    }

    fn fetch_request() -> actix_test::TestRequest {
        actix_test::TestRequest::post().uri("/api/plugin-bundles/fetch")
            .set_json(serde_json::json!({"url": "https://plugins.example/plugin.rimbundle", "sha256": sha256_hex(BUNDLE)}))
    }

    #[actix_web::test]
    async fn link_fetch_needs_a_session_and_csrf_and_uses_its_own_feature_gate() {
        let id = UserId::new();
        let mut service = MockPluginBundleService::new();
        service
            .expect_fetch()
            .times(1)
            .withf(move |cmd| cmd.actor.user_id == id && cmd.sha256 == sha256_hex(BUNDLE))
            .returning(|_| Box::pin(async { Ok(BUNDLE.to_vec()) }));
        let gate = rim_infra_services::feature_gate::InstanceFeatureGate::new(
            &[ServerFeature::WorkspacePlugins],
            &[],
        );
        let app = test_app!(authenticator(id), service, gate);
        assert_eq!(
            actix_test::call_service(&app, fetch_request().to_request())
                .await
                .status(),
            401
        );
        assert_eq!(
            actix_test::call_service(&app, signed_in(fetch_request(), false).to_request())
                .await
                .status(),
            403
        );
        let response =
            actix_test::call_service(&app, signed_in(fetch_request(), true).to_request()).await;
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        assert_eq!(actix_test::read_body(response).await, BUNDLE);
    }

    #[actix_web::test]
    async fn link_fetch_feature_can_be_disabled_independently() {
        let gate = rim_infra_services::feature_gate::InstanceFeatureGate::new(
            &[ServerFeature::PluginLinkFetch],
            &[],
        );
        let app = test_app!(
            authenticator(UserId::new()),
            MockPluginBundleService::new(),
            gate
        );
        let response =
            actix_test::call_service(&app, signed_in(fetch_request(), true).to_request()).await;
        assert_eq!(response.status(), 403);
        let body: serde_json::Value = actix_test::read_body_json(response).await;
        assert_eq!(body["feature"], "plugin_link_fetch");
    }

    #[actix_web::test]
    async fn link_fetch_returns_validation_errors_without_bundle_bytes() {
        let mut service = MockPluginBundleService::new();
        service.expect_fetch().returning(|_| {
            Box::pin(async {
                Err(ServiceError::Validation(
                    "bundle SHA-256 mismatch".to_string(),
                ))
            })
        });
        let app = test_app!(authenticator(UserId::new()), service);
        let response =
            actix_test::call_service(&app, signed_in(fetch_request(), true).to_request()).await;
        assert_eq!(response.status(), 422);
        let body: serde_json::Value = actix_test::read_body_json(response).await;
        assert_eq!(body["error"], "bundle SHA-256 mismatch");
    }

    #[actix_web::test]
    async fn upload_answers_201_then_200_with_the_hash_computed_while_reading() {
        let server_id = ServerId::new();
        let user_id = UserId::new();
        let mut service = MockPluginBundleService::new();
        let mut first = true;
        service
            .expect_upload()
            .withf(move |cmd| {
                cmd.sha256 == sha256_hex(BUNDLE)
                    && cmd.bytes == BUNDLE
                    && cmd.server_id == server_id
                    && cmd.actor.user_id == user_id
            })
            .returning(move |_| {
                let created = std::mem::replace(&mut first, false);
                Box::pin(async move { Ok(uploaded(server_id, created)) })
            });
        let app = test_app!(authenticator(user_id), service);

        let resp = actix_test::call_service(
            &app,
            signed_in(upload_request(server_id, BUNDLE), true).to_request(),
        )
        .await;
        assert_eq!(resp.status(), 201);
        let body: serde_json::Value = actix_test::read_body_json(resp).await;
        assert_eq!(body["sha256"], sha256_hex(BUNDLE));
        assert_eq!(body["size"], BUNDLE.len());
        assert_eq!(
            body["url"],
            format!("/api/plugin-bundles/{server_id}/{}", sha256_hex(BUNDLE))
        );

        let resp = actix_test::call_service(
            &app,
            signed_in(upload_request(server_id, BUNDLE), true).to_request(),
        )
        .await;
        assert_eq!(resp.status(), 200);
    }

    #[actix_web::test]
    async fn upload_needs_a_session_then_the_csrf_header() {
        let server_id = ServerId::new();
        let mut service = MockPluginBundleService::new();
        service.expect_upload().never();
        let app = test_app!(authenticator(UserId::new()), service);

        let resp =
            actix_test::call_service(&app, upload_request(server_id, BUNDLE).to_request()).await;
        assert_eq!(resp.status(), 401);
        let resp = actix_test::call_service(
            &app,
            signed_in(upload_request(server_id, BUNDLE), false).to_request(),
        )
        .await;
        assert_eq!(resp.status(), 403);
        // A header that does not match the cookie is no better.
        let resp = actix_test::call_service(
            &app,
            signed_in(upload_request(server_id, BUNDLE), false)
                .insert_header(("x-csrf-token", "forged"))
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), 403);
    }

    #[actix_web::test]
    async fn upload_refuses_the_wrong_media_type_and_oversized_bodies() {
        let server_id = ServerId::new();
        let mut service = MockPluginBundleService::new();
        service.expect_upload().never();
        let app = test_app!(authenticator(UserId::new()), service);

        let resp = actix_test::call_service(
            &app,
            signed_in(
                actix_test::TestRequest::post()
                    .uri(&format!("/api/plugin-bundles/{server_id}"))
                    .insert_header((header::CONTENT_TYPE, "application/json"))
                    .set_payload(BUNDLE),
                true,
            )
            .to_request(),
        )
        .await;
        assert_eq!(resp.status(), 415);

        let oversized = vec![b' '; MAX_PLUGIN_BUNDLE_BYTES + 1];
        let resp = actix_test::call_service(
            &app,
            signed_in(upload_request(server_id, oversized), true).to_request(),
        )
        .await;
        assert_eq!(resp.status(), 413);
    }

    #[actix_web::test]
    async fn upload_maps_refusals_to_their_codes() {
        let server_id = ServerId::new();
        for (error, status, message) in [
            (ServiceError::PermissionDenied, 403, None),
            (
                ServiceError::Validation("rim-plugin.json is missing".into()),
                422,
                Some("rim-plugin.json is missing"),
            ),
            (
                ServiceError::not_in_direct_space(),
                400,
                Some(NOT_IN_DIRECT_SPACE),
            ),
            (
                ServiceError::RateLimited {
                    retry_after_seconds: 60,
                },
                429,
                None,
            ),
        ] {
            let mut service = MockPluginBundleService::new();
            let mut error = Some(error);
            service.expect_upload().returning(move |_| {
                let error = error.take().expect("one call");
                Box::pin(async move { Err(error) })
            });
            let app = test_app!(authenticator(UserId::new()), service);
            let resp = actix_test::call_service(
                &app,
                signed_in(upload_request(server_id, BUNDLE), true).to_request(),
            )
            .await;
            assert_eq!(resp.status(), status);
            if let Some(message) = message {
                let body: serde_json::Value = actix_test::read_body_json(resp).await;
                assert_eq!(body["error"], message);
            }
        }
    }

    #[actix_web::test]
    async fn download_serves_the_bytes_with_the_bundle_headers() {
        let server_id = ServerId::new();
        let sha256 = sha256_hex(BUNDLE);
        let mut service = MockPluginBundleService::new();
        let expected = sha256.clone();
        service
            .expect_download()
            .withf(move |q| q.server_id == server_id && q.sha256 == expected)
            .returning(|_| Box::pin(async { Ok(BUNDLE.to_vec()) }));
        let app = test_app!(authenticator(UserId::new()), service);

        let req = signed_in(
            actix_test::TestRequest::get()
                .uri(&format!("/api/plugin-bundles/{server_id}/{sha256}")),
            false,
        );
        let resp = actix_test::call_service(&app, req.to_request()).await;
        assert_eq!(resp.status(), 200);
        let headers = resp.headers();
        assert_eq!(
            headers.get(header::CONTENT_TYPE).expect("header"),
            PLUGIN_BUNDLE_MEDIA_TYPE
        );
        assert_eq!(
            headers.get(header::X_CONTENT_TYPE_OPTIONS).expect("header"),
            "nosniff"
        );
        assert_eq!(
            headers.get(header::CONTENT_DISPOSITION).expect("header"),
            "attachment"
        );
        assert_eq!(
            headers.get(header::CACHE_CONTROL).expect("header"),
            "private, max-age=31536000, immutable"
        );
        assert_eq!(actix_test::read_body(resp).await, BUNDLE);
    }

    #[actix_web::test]
    async fn download_answers_404_for_non_members_and_unknown_hashes() {
        let server_id = ServerId::new();
        let mut service = MockPluginBundleService::new();
        service
            .expect_download()
            .returning(|_| Box::pin(async { Err(ServiceError::NotFound) }));
        let app = test_app!(authenticator(UserId::new()), service);
        for uri in [
            format!("/api/plugin-bundles/{server_id}/{}", sha256_hex(BUNDLE)),
            format!("/api/plugin-bundles/not-a-uuid/{}", sha256_hex(BUNDLE)),
        ] {
            let req = signed_in(actix_test::TestRequest::get().uri(&uri), false);
            let resp = actix_test::call_service(&app, req.to_request()).await;
            assert_eq!(resp.status(), 404, "{uri}");
        }
        let resp = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri(&format!(
                    "/api/plugin-bundles/{server_id}/{}",
                    sha256_hex(BUNDLE)
                ))
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), 401);
    }

    /// With workspace plugins disabled, upload and download answer 403 `FEATURE_DISABLED`
    /// before the session is looked at; the unprimed mocks panic on any call.
    #[actix_web::test]
    async fn a_disabled_feature_refuses_upload_and_download() {
        let gate = rim_infra_services::feature_gate::InstanceFeatureGate::new(
            &[ServerFeature::WorkspacePlugins],
            &[],
        );
        let app = test_app!(
            MockRequestAuthenticator::new(),
            MockPluginBundleService::new(),
            gate
        );
        let server_id = ServerId::new();
        for req in [
            signed_in(upload_request(server_id, BUNDLE), true).to_request(),
            actix_test::TestRequest::get()
                .uri(&format!(
                    "/api/plugin-bundles/{server_id}/{}",
                    sha256_hex(BUNDLE)
                ))
                .to_request(),
        ] {
            let resp = actix_test::call_service(&app, req).await;
            assert_eq!(resp.status(), 403);
            let body: serde_json::Value = actix_test::read_body_json(resp).await;
            assert_eq!(body["code"], "FEATURE_DISABLED");
            assert_eq!(body["feature"], "workspace_plugins");
        }
    }
}
