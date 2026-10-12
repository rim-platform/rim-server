use crate::app::{S2sRuntime, build_app_runtime};
use crate::config::RIMConfig;
use crate::edge::{EdgeLimits, MutationCaller};
use crate::file_handlers::{
    handle_attachment_download, handle_avatar_upload, handle_file_download, handle_file_upload,
    handle_report_attachment_download,
};
use crate::graphql_support::{
    apply_auth_cookies, count_mutation_fields, enforce_csrf_for_batch, extract_auth,
    internal_graphql_error_response,
};
use crate::health::{ReadinessCache, SystemProbe};
use crate::plugin_bundle_handlers::{handle_plugin_bundle_download, handle_plugin_bundle_upload};
use actix_web::{App, Either, HttpRequest, HttpResponse, HttpServer, guard, web};
use async_graphql_actix_web::{GraphQLBatchRequest, GraphQLResponse};
use rim_api::AppSchema;
use rim_api::auth_intent::AuthIntentCtx;
use rim_api::graphql::index_graphiql;
use rim_api::session_guard::ConnectionSession;
use rim_api::subscription_limit::SubscriptionSlots;
use rim_api::subscriptions_only::WebSocketConnection;
use tracing::{Instrument, info, info_span};

use std::sync::Arc;

use rim_services::auth::service::RequestAuthenticator;
use rim_services::session_revocation::{SessionRevocations, WatchedSession};

use rim_api::auth_context::AuthActorCtx;
use uuid::Uuid;

/// Subset of config values needed at request time.
#[derive(Clone)]
struct HandlerConfig {
    session_ttl_days: u32,
    /// Whether to read `X-Forwarded-For` for real-IP resolution.
    /// Only `true` when `RIM_TRUST_FORWARDED_HEADERS=true`.
    trust_forwarded_headers: bool,
    /// Whether to unconditionally set `Secure` on all cookies.
    /// Only `true` when `RIM_FORCE_SECURE_COOKIES=true`.
    force_secure_cookies: bool,
}

/// A server listening and ready to run, with the handles its tests and the CLI need.
pub struct RunningServer {
    /// The HTTP server; await it to run until shutdown.
    pub server: actix_web::dev::Server,
    /// Server identity and server-to-server traffic.
    pub s2s: S2sRuntime,
    /// The GraphQL schema the server runs, for tests that drive subscriptions directly.
    pub schema: AppSchema,
    /// Keeps the background tasks running while the server is alive.
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// The server's connection pool, for closing once the server has stopped.
    pub db: sea_orm::DatabaseConnection,
}

/// Bind `RIM_SERVER_HOST:RIM_SERVER_PORT` and run until shutdown.
pub async fn start_server(config: &RIMConfig) -> std::io::Result<()> {
    let listener = std::net::TcpListener::bind((config.server_host.as_str(), config.server_port))?;
    let running = serve(config, listener).await?;
    let result = running.server.await;
    drop(running.shutdown_tx);
    if let Err(e) = running.db.close().await {
        tracing::warn!(error = %e, "could not close the database pool");
    }
    result
}

/// Build the application and serve it on `listener`. The returned server runs once awaited
/// or spawned.
pub async fn serve(
    config: &RIMConfig,
    listener: std::net::TcpListener,
) -> std::io::Result<RunningServer> {
    info!("Starting RIM server...");

    tracing::debug!(
        host = %config.server_host,
        port = %config.server_port,
        session_ttl_days = %config.session_ttl_days,
        graphiql = %config.enable_graphiql,
        "Configuration loaded"
    );
    let enable_graphiql = config.enable_graphiql;
    let allowed_origins = crate::origin::AllowedOrigins::from_config(config);
    let handler_config = HandlerConfig {
        session_ttl_days: config.session_ttl_days,
        trust_forwarded_headers: config.trust_forwarded_headers(),
        force_secure_cookies: config.force_secure_cookies(),
    };

    let metrics_addr = config.metrics_addr().map_err(std::io::Error::other)?;
    let runtime = build_app_runtime(config).await?;
    let edge_limits = web::Data::new(
        EdgeLimits::new(config.api_limits(), config.trust_forwarded_headers())
            .with_deny_list(runtime.ip_deny_list),
    );
    let schema: AppSchema = runtime.schema;
    let schema_handle = schema.clone();
    let request_authenticator = runtime.request_authenticator;
    let session_revocations = runtime.session_revocations;
    let storage_backend = runtime.storage_backend;
    let storage_service = runtime.storage_service;
    let message_attachment_service = runtime.message_attachment_service;
    let moderation_service = runtime.moderation_service;
    let plugin_bundle_service = runtime.plugin_bundle_service;
    let feature_gate = runtime.feature_gate;
    let readiness = web::Data::new(ReadinessCache::new(Arc::new(SystemProbe {
        db: runtime.db.clone(),
        server_key_file: config.key_files().key_file,
        franking_key_file: config.franking_key_file(),
    })));
    let s2s_runtime = runtime.s2s;
    let s2s_state = s2s_runtime.state.clone();
    let shutdown_tx = runtime.shutdown_tx;
    if let Some(addr) = metrics_addr {
        crate::metrics::start(addr, runtime.db.clone(), shutdown_tx.subscribe())?;
        rim_infra_services::metrics::set_audit_retention_days(
            config.audit_retention_days().ok().flatten(),
        );
    }
    let db = runtime.db;

    let server = HttpServer::new(move || {
        let schema = schema.clone();
        let handler_cfg = handler_config.clone();

        let cors = allowed_origins.clone().cors();

        // The per-address limit sits inside CORS, so a refusal carries the CORS headers and a
        // preflight is not counted.
        let mut api = web::scope("")
            .wrap(actix_web::middleware::from_fn(crate::edge::limit_api))
            .wrap(cors)
            .service(
                web::resource("/api")
                    .guard(guard::Post())
                    .to(graphql_handler),
            )
            .service(web::resource("/api/ws").route(web::get().to(graphql_ws_handler)))
            .service(web::resource("/api/files/upload").route(web::post().to(handle_file_upload)))
            .service(web::resource("/api/files/avatar").route(web::post().to(handle_avatar_upload)))
            .service(
                web::resource("/api/attachments/{attachment_id}/download")
                    .route(web::get().to(handle_attachment_download)),
            )
            .service(
                web::resource("/api/reports/{report_id}/attachments/{blob_sha256}")
                    .route(web::get().to(handle_report_attachment_download)),
            )
            .service(
                web::resource("/api/files/{path:.*}").route(web::get().to(handle_file_download)),
            )
            .service(
                web::resource("/api/plugin-bundles/{server_id}")
                    .route(web::post().to(handle_plugin_bundle_upload)),
            )
            .service(
                web::resource("/api/plugin-bundles/{server_id}/{sha256}")
                    .route(web::get().to(handle_plugin_bundle_download)),
            )
            .service(web::resource("/api/sdl").route(web::get().to(sdl_handler)))
            .service(
                web::resource("/api/remote/{host_server_id}/{tail:.*}")
                    .route(web::get().to(crate::remote_handlers::handle_remote))
                    .route(web::post().to(crate::remote_handlers::handle_remote)),
            );

        if enable_graphiql {
            api = api.service(index_graphiql);
        }

        // Server-to-server routes sit outside CORS: peers send no `Origin`, and the discovery
        // document is readable from any origin. The health probes sit there too: no session,
        // no CSRF, no `Origin` needed. The deny list covers every route but `/healthz`, inside
        // the request metrics so its refusals are counted.
        App::new()
            .wrap(actix_web::middleware::from_fn(crate::edge::deny_blocked))
            .wrap(actix_web::middleware::from_fn(crate::metrics::record_http))
            .service(
                web::resource("/.well-known/rim-server")
                    .route(web::get().to(crate::s2s::discovery::handle)),
            )
            .service(web::resource("/healthz").route(web::get().to(crate::health::healthz)))
            .service(web::resource("/readyz").route(web::get().to(crate::health::readyz)))
            .service(web::resource("/s2s/v1/{tail:.*}").to(crate::s2s::handle))
            .service(api)
            .app_data(web::Data::new(s2s_state.clone()))
            .app_data(readiness.clone())
            .app_data(web::Data::new(schema))
            .app_data(web::Data::new(Arc::clone(&request_authenticator)))
            .app_data(web::Data::new(Arc::clone(&session_revocations)))
            .app_data(web::Data::new(Arc::clone(&storage_backend)))
            .app_data(web::Data::new(Arc::clone(&storage_service)))
            .app_data(web::Data::new(Arc::clone(&message_attachment_service)))
            .app_data(web::Data::new(Arc::clone(&moderation_service)))
            .app_data(web::Data::new(Arc::clone(&plugin_bundle_service)))
            .app_data(web::Data::new(Arc::clone(&feature_gate)))
            .app_data(web::Data::new(handler_cfg))
            .app_data(edge_limits.clone())
    })
    .listen(listener)?
    .run();
    Ok(RunningServer {
        server,
        schema: schema_handle,
        s2s: s2s_runtime,
        shutdown_tx,
        db,
    })
}

/// GET /api/sdl — export the GraphQL schema in SDL format for codegen tooling.
async fn sdl_handler(schema: web::Data<AppSchema>) -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/plain; charset=utf-8")
        .body(schema.sdl())
}

async fn graphql_handler(
    schema: web::Data<AppSchema>,
    handler_cfg: web::Data<HandlerConfig>,
    edge_limits: web::Data<EdgeLimits>,
    req: HttpRequest,
    gql: GraphQLBatchRequest,
) -> Either<GraphQLResponse, HttpResponse> {
    let batch = gql.into_inner();
    if let async_graphql::BatchRequest::Batch(requests) = &batch
        && edge_limits.batch_too_large(requests.len())
    {
        return Either::Right(crate::edge::batch_too_large_response());
    }
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| Uuid::now_v7().to_string());

    let has_mutation = batch_has_mutation(&batch);
    let span = info_span!(
        "graphql.request",
        request_id = %request_id,
        batch_size = %batch.iter().count(),
        has_mutation = %has_mutation
    );

    let response = async move {
        // CSRF is enforced once at the HTTP level for the entire batch.
        if let Err(resp) = enforce_csrf_for_batch(&req, &batch) {
            return resp;
        }
        // `Schema::data` is only available via GraphQL execution context, so we inject auth here via
        // AppData instead.
        let request_authenticator = req
            .app_data::<web::Data<Arc<dyn RequestAuthenticator>>>()
            .map(|d| Arc::clone(d.get_ref()));

        let auth = match request_authenticator {
            Some(authenticator) => match extract_auth(&req, authenticator.as_ref()).await {
                Ok(v) => v,
                Err(err) => {
                    tracing::error!(request_id = %request_id, error = ?err, "failed to extract auth session");
                    return internal_graphql_error_response("failed to validate session");
                }
            },
            None => Default::default(),
        };
        let client_ip = crate::edge::client_ip(&req, handler_cfg.trust_forwarded_headers);
        let caller = match (auth.session_id, client_ip) {
            (Some(session_id), _) => Some(MutationCaller::Session(session_id)),
            (None, Some(ip)) => Some(MutationCaller::Address(ip)),
            (None, None) => None,
        };
        if let Some(caller) = caller
            && let Err(retry_after_seconds) =
                edge_limits.spend_mutations(&caller, count_mutation_fields(&batch))
        {
            return rate_limited_graphql_response(retry_after_seconds);
        }
        if let Some(user_id) = auth.user_id
            && let Some(response) = refuse_remote_account_mutation(&req, &batch, user_id).await
        {
            return response;
        }
        let actor_ctx = AuthActorCtx::new(auth.user_id, auth.session_id)
            .with_device_key_id(auth.device_key_id);
        let actor_user_id = actor_ctx.actor().map(|u| u.to_string());

        // The client address for the rate-limiting resolvers (e.g. previewInviteToken): the
        // proxy headers count only with RIM_TRUST_FORWARDED_HEADERS=true, since without a
        // proxy in front a client could forge them.
        let remote_ip = rim_api::auth_context::RemoteIp::new(
            client_ip.map_or_else(|| "unknown".to_string(), |ip| ip.to_string()),
        );

        // Inject auth context and auth intent into each request in the batch.
        // Auth intent is shared across the batch so cookies are set once.
        let auth_intent = Arc::new(AuthIntentCtx::default());
        let batch = batch
            .data(actor_ctx)
            .data(Arc::clone(&auth_intent))
            .data(remote_ip);

        let resp = schema.execute_batch(batch).await;

        log_batch_errors(&resp, &request_id, actor_user_id.as_deref());

        let resp = annotate_batch_request_id(resp, &request_id);

        let gql_resp = GraphQLResponse::from(resp);
        apply_auth_cookies(
            gql_resp,
            auth_intent.as_ref(),
            &req,
            handler_cfg.session_ttl_days,
            handler_cfg.trust_forwarded_headers,
            handler_cfg.force_secure_cookies,
        )
    }
    .instrument(span)
    .await;
    Either::Left(response)
}

/// The GraphQL answer to a request over its mutation budget: `RATE_LIMITED` with
/// `retryAfterSeconds`.
fn rate_limited_graphql_response(retry_after_seconds: u64) -> GraphQLResponse {
    let error = rim_api::errors::map_service_error(rim_services::ServiceError::RateLimited {
        retry_after_seconds,
    });
    GraphQLResponse::from(async_graphql::Response::from_errors(vec![
        error.into_server_error(async_graphql::Pos::default()),
    ]))
}

/// `FORBIDDEN` when someone whose relay is another server, signed in here directly, sends a
/// mutation that changes their account rather than a space: those run at their relay.
async fn refuse_remote_account_mutation(
    req: &HttpRequest,
    batch: &async_graphql::BatchRequest,
    user_id: rim_core::user::model::UserId,
) -> Option<GraphQLResponse> {
    let field = batch.iter().find_map(|request| {
        rim_api::federation::refused_account_mutation(
            &request.query,
            request.operation_name.as_deref(),
        )
    })?;
    let state = req.app_data::<web::Data<crate::s2s::S2sState>>()?;
    match state.host.is_remote_user(user_id).await {
        Ok(false) => None,
        Ok(true) => {
            tracing::info!(field, user_id = %user_id, "refused an account mutation for a remote member signed in directly");
            let mut response =
                async_graphql::Response::from_errors(vec![async_graphql::ServerError::new(
                    "not available to remote members",
                    None,
                )]);
            for error in &mut response.errors {
                error
                    .extensions
                    .get_or_insert_with(Default::default)
                    .set("code", "FORBIDDEN");
            }
            Some(GraphQLResponse::from(response))
        }
        Err(e) => {
            tracing::error!(error = %e, "could not tell whether a user is remote");
            Some(internal_graphql_error_response(
                "failed to validate session",
            ))
        }
    }
}

async fn graphql_ws_handler(
    schema: web::Data<AppSchema>,
    req: HttpRequest,
    payload: web::Payload,
) -> Result<HttpResponse, actix_web::Error> {
    // Extract auth from the HTTP upgrade request — same as graphql_handler does —
    // so that `require_actor(ctx)` works inside subscription resolvers.
    let request_authenticator = req
        .app_data::<web::Data<Arc<dyn RequestAuthenticator>>>()
        .map(|d| Arc::clone(d.get_ref()));

    let auth = match &request_authenticator {
        Some(authenticator) => match extract_auth(&req, authenticator.as_ref()).await {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(error = ?err, "WS: failed to extract auth session");
                return Err(actix_web::error::ErrorUnauthorized(
                    "failed to validate session",
                ));
            }
        },
        None => Default::default(),
    };

    // Defense-in-depth: reject unauthenticated users at the transport level
    // so that even if a subscription resolver forgets `require_actor`, the
    // WebSocket connection is never established for anonymous users.
    let (Some(actor), Some(session_id)) = (auth.user_id, auth.session_id) else {
        return Err(actix_web::error::ErrorUnauthorized(
            "authentication required",
        ));
    };

    let actor_ctx =
        AuthActorCtx::new(Some(actor), Some(session_id)).with_device_key_id(auth.device_key_id);

    let mut data = async_graphql::Data::default();
    data.insert(actor_ctx);
    // The socket runs subscriptions only; queries and mutations go through POST /api.
    data.insert(WebSocketConnection);
    // The connection ends with its session: on revocation through the registry, at expiry by
    // its own timer, and before each operation the session is checked again.
    let revocations = req
        .app_data::<web::Data<Arc<dyn SessionRevocations>>>()
        .map(|d| Arc::clone(d.get_ref()));
    let watch = revocations.map(|revocations| {
        if let Some(authenticator) = &request_authenticator {
            data.insert(ConnectionSession::new(
                session_id,
                Arc::clone(authenticator),
                Arc::clone(&revocations),
            ));
        }
        revocations.watch(WatchedSession {
            session_id,
            user_id: actor,
            device_key_id: auth.device_key_id,
        })
    });
    if let Some(edge_limits) = req.app_data::<web::Data<EdgeLimits>>() {
        // The connection slot lives in the connection's data, so it is given back when the
        // connection ends.
        match edge_limits.open_ws(actor) {
            Ok(Some(slot)) => data.insert(slot),
            Ok(None) => {}
            Err(()) => {
                return Ok(HttpResponse::TooManyRequests()
                    .json(serde_json::json!({ "error": "too_many_connections" })));
            }
        }
        if let Some(limit) = edge_limits.subscriptions_per_connection() {
            data.insert(SubscriptionSlots::new(limit, || {
                rim_infra_services::metrics::rate_limited("ws_subscriptions");
            }));
        }
    }

    crate::ws::start(
        schema.into_inner().as_ref().clone(),
        data,
        crate::ws::SessionBound {
            watch,
            expires_at: auth.expires_at,
        },
        &req,
        payload,
    )
}

fn request_has_mutation(req: &async_graphql::Request) -> bool {
    let doc = match async_graphql_parser::parse_query(req.query.as_str()) {
        Ok(d) => d,
        Err(_) => return false,
    };

    doc.operations.iter().any(|(_, op)| {
        matches!(
            op.node.ty,
            async_graphql_parser::types::OperationType::Mutation
        )
    })
}

fn batch_has_mutation(batch: &async_graphql::BatchRequest) -> bool {
    batch.iter().any(request_has_mutation)
}

fn log_batch_errors(
    resp: &async_graphql::BatchResponse,
    request_id: &str,
    actor_user_id: Option<&str>,
) {
    let responses: Vec<&async_graphql::Response> = match resp {
        async_graphql::BatchResponse::Single(r) => vec![r],
        async_graphql::BatchResponse::Batch(rs) => rs.iter().collect(),
    };

    for resp in responses {
        for e in &resp.errors {
            let code = e
                .extensions
                .as_ref()
                .and_then(|ext| ext.get("code"))
                .and_then(|v| match v {
                    async_graphql::Value::String(s) => Some(s.as_str()),
                    async_graphql::Value::Enum(n) => Some(n.as_str()),
                    _ => None,
                })
                .unwrap_or("UNKNOWN");

            if code == "INTERNAL" || code == "UNKNOWN" {
                if let Some(src) = &e.source {
                    tracing::error!(
                        request_id = %request_id,
                        actor_user_id = ?actor_user_id,
                        code = %code,
                        source = ?src,
                        "graphql internal error source"
                    );
                }
                tracing::error!(
                    request_id = %request_id,
                    actor_user_id = ?actor_user_id,
                    code = %code,
                    message = %e.message,
                    path = ?e.path,
                    locations = ?e.locations,
                    extensions = ?e.extensions,
                    "graphql error"
                );
            } else {
                tracing::warn!(
                    request_id = %request_id,
                    actor_user_id = ?actor_user_id,
                    code = %code,
                    message = %e.message,
                    path = ?e.path,
                    locations = ?e.locations,
                    extensions = ?e.extensions,
                    "graphql error"
                );
            }
        }
    }
}

fn annotate_batch_request_id(
    resp: async_graphql::BatchResponse,
    request_id: &str,
) -> async_graphql::BatchResponse {
    match resp {
        async_graphql::BatchResponse::Single(mut r) => {
            for e in &mut r.errors {
                let ext = e.extensions.get_or_insert_with(Default::default);
                if ext.get("request_id").is_none() {
                    ext.set("request_id", request_id.to_owned());
                }
            }
            async_graphql::BatchResponse::Single(r)
        }
        async_graphql::BatchResponse::Batch(mut rs) => {
            for r in &mut rs {
                for e in &mut r.errors {
                    let ext = e.extensions.get_or_insert_with(Default::default);
                    if ext.get("request_id").is_none() {
                        ext.set("request_id", request_id.to_owned());
                    }
                }
            }
            async_graphql::BatchResponse::Batch(rs)
        }
    }
}
