use crate::app::build_app_runtime;
use crate::config::RIMConfig;
use crate::file_handlers::{
    handle_attachment_download, handle_avatar_upload, handle_file_download, handle_file_upload,
};
use crate::graphql_support::{
    apply_auth_cookies, enforce_csrf_for_batch, extract_actor_and_session_id,
    internal_graphql_error_response,
};
use actix_web::{App, HttpRequest, HttpResponse, HttpServer, guard, web};
use async_graphql_actix_web::{GraphQLBatchRequest, GraphQLResponse, GraphQLSubscription};
use log::info;
use rim_api::AppSchema;
use rim_api::auth_intent::AuthIntentCtx;
use rim_api::graphql::index_graphiql;
use tracing::{Instrument, info_span};

use std::sync::Arc;

use rim_services::auth::service::RequestAuthenticator;

use rim_api::auth_context::AuthActorCtx;
use uuid::Uuid;

/// Subset of config values needed at request time.
#[derive(Clone)]
struct HandlerConfig {
    session_ttl_days: u32,
}

pub async fn start_server(config: &RIMConfig) -> std::io::Result<()> {
    info!("Starting RIM server...");

    tracing::debug!(
        host = %config.server_host,
        port = %config.server_port,
        session_ttl_days = %config.session_ttl_days,
        graphiql = %config.enable_graphiql,
        "Configuration loaded"
    );
    let bind_host = format!("{}:{}", config.server_host, config.server_port);
    let enable_graphiql = config.enable_graphiql;
    let cors_allowed_origin = config.cors_allowed_origin.clone();
    let handler_config = HandlerConfig {
        session_ttl_days: config.session_ttl_days,
    };

    let runtime = build_app_runtime(config).await?;
    let schema: AppSchema = runtime.schema;
    let request_authenticator = runtime.request_authenticator;
    let storage_backend = runtime.storage_backend;
    let storage_service = runtime.storage_service;
    let message_attachment_repo = runtime.message_attachment_repo;
    let message_repo = runtime.message_repo;
    let conversation_repo = runtime.conversation_repo;
    let conversation_member_repo = runtime.conversation_member_repo;
    let server_member_repo = runtime.server_member_repo;

    HttpServer::new(move || {
        let schema = schema.clone();
        let handler_cfg = handler_config.clone();

        let cors = match cors_allowed_origin {
            Some(ref origin) => actix_cors::Cors::default()
                .allowed_origin(origin)
                .allowed_methods(vec!["GET", "POST", "OPTIONS"])
                .allowed_headers(vec![
                    "Content-Type",
                    "Authorization",
                    "x-csrf-token",
                    "x-request-id",
                ])
                .supports_credentials()
                .max_age(3600),
            None => {
                // No CORS origin configured: block all cross-origin requests.
                // The browser will deny requests from any non-same-origin page
                // because no Access-Control-Allow-Origin header is emitted.
                actix_cors::Cors::default()
                    .allowed_origin("https://0.0.0.0")
                    .block_on_origin_mismatch(true)
            }
        };

        let mut app = App::new()
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
                web::resource("/api/files/{path:.*}").route(web::get().to(handle_file_download)),
            )
            .service(web::resource("/api/sdl").route(web::get().to(sdl_handler)));

        if enable_graphiql {
            app = app.service(index_graphiql);
        }

        app.app_data(web::Data::new(schema))
            .app_data(web::Data::new(Arc::clone(&request_authenticator)))
            .app_data(web::Data::new(Arc::clone(&storage_backend)))
            .app_data(web::Data::new(Arc::clone(&storage_service)))
            .app_data(web::Data::new(Arc::clone(&message_attachment_repo)))
            .app_data(web::Data::new(Arc::clone(&message_repo)))
            .app_data(web::Data::new(Arc::clone(&conversation_repo)))
            .app_data(web::Data::new(Arc::clone(&conversation_member_repo)))
            .app_data(web::Data::new(Arc::clone(&server_member_repo)))
            .app_data(web::Data::new(handler_cfg))
    })
    .bind(bind_host)?
    .run()
    .await
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
    req: HttpRequest,
    gql: GraphQLBatchRequest,
) -> GraphQLResponse {
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| Uuid::now_v7().to_string());

    let batch = gql.into_inner();
    let has_mutation = batch_has_mutation(&batch);
    let span = info_span!(
        "graphql.request",
        request_id = %request_id,
        batch_size = %batch.iter().count(),
        has_mutation = %has_mutation
    );

    async move {
        // CSRF is enforced once at the HTTP level for the entire batch.
        if let Err(resp) = enforce_csrf_for_batch(&req, &batch) {
            return resp;
        }
        // `Schema::data` is only available via GraphQL execution context, so we inject auth here via
        // AppData instead.
        let request_authenticator = req
            .app_data::<web::Data<Arc<dyn RequestAuthenticator>>>()
            .map(|d| Arc::clone(d.get_ref()));

        let (actor, session_id) = match request_authenticator {
            Some(authenticator) => match extract_actor_and_session_id(&req, authenticator.as_ref()).await {
                Ok(v) => v,
                Err(err) => {
                    tracing::error!(request_id = %request_id, error = ?err, "failed to extract auth session");
                    return internal_graphql_error_response("failed to validate session");
                }
            },
            None => (None, None),
        };
        let actor_ctx = AuthActorCtx::new(actor, session_id);
        let actor_user_id = actor_ctx.actor().map(|u| u.to_string());

        // Inject auth context and auth intent into each request in the batch.
        // Auth intent is shared across the batch so cookies are set once.
        let auth_intent = Arc::new(AuthIntentCtx::default());
        let batch = batch.data(actor_ctx).data(Arc::clone(&auth_intent));

        let resp = schema.execute_batch(batch).await;

        log_batch_errors(&resp, &request_id, actor_user_id.as_deref());

        let resp = annotate_batch_request_id(resp, &request_id);

        let gql_resp = GraphQLResponse::from(resp);
        apply_auth_cookies(gql_resp, auth_intent.as_ref(), &req, handler_cfg.session_ttl_days)
    }
    .instrument(span)
    .await
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

    let (actor, session_id) = match request_authenticator {
        Some(authenticator) => {
            match extract_actor_and_session_id(&req, authenticator.as_ref()).await {
                Ok(v) => v,
                Err(err) => {
                    tracing::error!(error = ?err, "WS: failed to extract auth session");
                    return Err(actix_web::error::ErrorUnauthorized(
                        "failed to validate session",
                    ));
                }
            }
        }
        None => (None, None),
    };

    // Defense-in-depth: reject unauthenticated users at the transport level
    // so that even if a subscription resolver forgets `require_actor`, the
    // WebSocket connection is never established for anonymous users.
    let Some(actor) = actor else {
        return Err(actix_web::error::ErrorUnauthorized(
            "authentication required",
        ));
    };

    let actor_ctx = AuthActorCtx::new(Some(actor), session_id);

    let mut data = async_graphql::Data::default();
    data.insert(actor_ctx);

    GraphQLSubscription::new(schema.into_inner().as_ref().clone())
        .with_data(data)
        .start(&req, payload)
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
