use crate::app::build_app_runtime;
use crate::config::RIMConfig;
use crate::graphql_support::{
    apply_auth_cookies, enforce_csrf, extract_actor_and_session_id, internal_graphql_error_response,
};
use actix_web::{App, HttpRequest, HttpServer, guard, web};
use async_graphql_actix_web::{GraphQLRequest, GraphQLResponse};
use log::{debug, info};
use rim_api::AppSchema;
use rim_api::auth_intent::insert_auth_intent_ctx;
use rim_api::graphql::index_graphiql;
use tracing::{Instrument, info_span};

use std::sync::Arc;

use rim_services::auth::service::RequestAuthenticator;

use rim_api::auth_context::AuthActorCtx;
use uuid::Uuid;

pub async fn start_server(config: &RIMConfig) -> std::io::Result<()> {
    info!("Starting RIM server...");

    debug!("Configuration: {:?}", config);
    let bind_host = format!("{}:{}", config.server_host, config.server_port);

    let runtime = build_app_runtime(config).await?;
    let schema: AppSchema = runtime.schema;
    let request_authenticator = runtime.request_authenticator;

    HttpServer::new(move || {
        let schema = schema.clone();
        App::new()
            .service(web::resource("/").guard(guard::Post()).to(graphql_handler))
            .service(index_graphiql)
            .app_data(web::Data::new(schema))
            .app_data(web::Data::new(Arc::clone(&request_authenticator)))
    })
    .bind(bind_host)?
    .run()
    .await
}

async fn graphql_handler(
    schema: web::Data<AppSchema>,
    req: HttpRequest,
    gql: GraphQLRequest,
) -> GraphQLResponse {
    let request_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| Uuid::now_v7().to_string());

    let op_name = gql.0.operation_name.clone().unwrap_or_default();
    let has_mutation = graphql_has_mutation(&gql.0);
    let span = info_span!(
        "graphql.request",
        request_id = %request_id,
        operation_name = %op_name,
        has_operation_name = %(!op_name.is_empty()),
        has_mutation = %has_mutation
    );

    async move {
        if let Err(resp) = enforce_csrf(&req, &gql.0) {
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

        let request = gql.into_inner().data(actor_ctx);
        let (request, auth_intent) = insert_auth_intent_ctx(request);
        let resp = schema.execute(request).await;

        if !resp.errors.is_empty() {
            for e in &resp.errors {
                // Server-side visibility: log full GraphQL error details.
                // Client response remains unchanged.
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

        let mut resp = resp;
        // Add request_id for correlation (safe to expose).
        for e in &mut resp.errors {
            let ext = e.extensions.get_or_insert_with(Default::default);
            if ext.get("request_id").is_none() {
                ext.set("request_id", request_id.clone());
            }
        }

        let gql_resp = GraphQLResponse::from(resp);
        apply_auth_cookies(gql_resp, auth_intent.as_ref(), &req)
    }
    .instrument(span)
    .await
}

fn graphql_has_mutation(req: &async_graphql::Request) -> bool {
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
