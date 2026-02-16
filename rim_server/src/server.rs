use crate::config::RIMConfig;
use actix_web::{App, HttpRequest, HttpServer, guard, web};
use async_graphql_actix_web::{GraphQLRequest, GraphQLResponse};
use log::{debug, info};
use rim_api::AppSchema;
use rim_api::auth_intent::{AuthIntent, AuthIntentCtx, insert_auth_intent_ctx};
use rim_api::build_schema;
use rim_api::graphql::index_graphiql;
use sea_orm::Database;

use std::sync::Arc;

use rim_auth::session_token::hash_token;
use rim_infra_db::sea_orm_impl;
use rim_infra_db::sea_orm_impl::{
    invite::SeaOrmInviteRepository, server::SeaOrmServerRepository,
    server_member::SeaOrmServerMemberRepository, session::SeaOrmSessionRepository,
    user::SeaOrmUserRepository,
};
use rim_infra_services::AppRepos;
use rim_infra_services::auth::DefaultAuthService;
use rim_services::auth::service::AuthService;

use rim_api::query::AuthActorCtx;

pub async fn start_server(config: &RIMConfig) -> std::io::Result<()> {
    info!("Starting RIM server...");

    debug!("Configuration: {:?}", config);
    let bind_host = format!("{}:{}", config.server_host, config.server_port);

    let db = Database::connect(&config.database_url)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

    let user_repo: Arc<dyn rim_core::user::repository::UserRepository> =
        Arc::new(SeaOrmUserRepository::new(db.clone()));
    let server_repo: Arc<dyn rim_core::server::repository::ServerRepository> =
        Arc::new(SeaOrmServerRepository::new(db.clone()));
    let server_member_repo: Arc<dyn rim_core::server_member::repository::ServerMemberRepository> =
        Arc::new(SeaOrmServerMemberRepository::new(db.clone()));
    let session_repo: Arc<dyn rim_core::auth::repository::SessionRepository> =
        Arc::new(SeaOrmSessionRepository::new(db.clone()));
    let invite_repo: Arc<dyn rim_core::auth::repository::InviteRepository> =
        Arc::new(SeaOrmInviteRepository::new(db.clone()));

    let repos = AppRepos {
        conversation_repo: Arc::new(
            sea_orm_impl::conversation::SeaOrmConversationRepository::new(db.clone()),
        ),
        conversation_member_repo: Arc::new(
            sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository::new(db.clone()),
        ),
        message_repo: Arc::new(sea_orm_impl::message::SeaOrmMessageRepository::new(
            db.clone(),
        )),
        message_attachment_repo: Arc::new(
            sea_orm_impl::message_attachment::SeaOrmMessageAttachmentRepository::new(db.clone()),
        ),
        message_thread_repo: Arc::new(
            sea_orm_impl::message_thread::SeaOrmMessageThreadRepository::new(db.clone()),
        ),
        server_repo: Arc::clone(&server_repo),
        server_member_repo: Arc::clone(&server_member_repo),
        user_repo: Arc::clone(&user_repo),
        session_repo: Arc::clone(&session_repo),
        invite_repo: Arc::clone(&invite_repo),
    };

    let auth_service: Arc<dyn AuthService> = Arc::new(DefaultAuthService::new(
        repos.for_auth(),
        config.bootstrap_secret.clone(),
        chrono::Duration::days(i64::from(config.session_ttl_days)),
    ));

    let schema: AppSchema = build_schema(Arc::clone(&auth_service));

    HttpServer::new(move || {
        let schema = schema.clone();
        App::new()
            .service(web::resource("/").guard(guard::Post()).to(graphql_handler))
            .service(index_graphiql)
            .app_data(web::Data::new(schema))
            .app_data(web::Data::new(Arc::clone(&session_repo)))
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
    if let Err(resp) = enforce_csrf(&req, &gql.0) {
        return resp;
    }
    // `Schema::data` is only available via GraphQL execution context, so we inject auth here via
    // AppData instead.
    let session_repo = req
        .app_data::<web::Data<Arc<dyn rim_core::auth::repository::SessionRepository>>>()
        .map(|d| Arc::clone(d.get_ref()));

    let (actor, session_id) = match session_repo {
        Some(repo) => extract_actor_and_session_id(&req, repo.as_ref()).await,
        None => (None, None),
    };
    let actor_ctx = AuthActorCtx::new(actor, session_id);

    let request = gql.into_inner().data(actor_ctx);
    let (request, auth_intent) = insert_auth_intent_ctx(request);
    let resp = schema.execute(request).await;

    let gql_resp = GraphQLResponse::from(resp);
    apply_auth_cookies(gql_resp, auth_intent.as_ref(), &req)
}

fn enforce_csrf(req: &HttpRequest, gql: &async_graphql::Request) -> Result<(), GraphQLResponse> {
    // Allowlist auth mutations that establish/clear session.
    const ALLOW: &[&str] = &["bootstrap", "login", "accept_invite", "logout"];

    // Best-effort parse to determine if the request contains any mutation operations.
    let doc = match async_graphql_parser::parse_query(gql.query.as_str()) {
        Ok(d) => d,
        Err(_) => {
            // If we can't parse, let GraphQL validation return an error.
            return Ok(());
        }
    };

    let has_mutation = doc.operations.iter().any(|(_, op)| {
        matches!(
            op.node.ty,
            async_graphql_parser::types::OperationType::Mutation
        )
    });

    if !has_mutation {
        return Ok(());
    }

    let Some(op_name) = gql.operation_name.as_deref() else {
        // Require operationName when any mutation is present so we can allowlist safely.
        return Err(GraphQLResponse::from(async_graphql::Response::from_errors(
            vec![async_graphql::ServerError::new(
                "missing operationName for mutation",
                None,
            )],
        )));
    };

    if ALLOW.contains(&op_name) {
        return Ok(());
    }

    let csrf_cookie = req.cookie(COOKIE_CSRF).map(|c| c.value().to_string());
    let csrf_header = req
        .headers()
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    match (csrf_cookie, csrf_header) {
        (Some(c), Some(h)) if c == h => Ok(()),
        _ => Err(GraphQLResponse::from(async_graphql::Response::from_errors(
            vec![async_graphql::ServerError::new(
                "csrf validation failed",
                None,
            )],
        ))),
    }
}

async fn extract_actor_and_session_id(
    req: &HttpRequest,
    session_repo: &dyn rim_core::auth::repository::SessionRepository,
) -> (
    Option<rim_core::user::model::UserId>,
    Option<rim_core::auth::model::SessionId>,
) {
    let cookie = match req.cookie(COOKIE_SESSION) {
        Some(c) => c,
        None => return (None, None),
    };

    let token_hash = hash_token(cookie.value());
    let sess = match session_repo.get_by_token_hash(&token_hash).await {
        Ok(s) => s,
        Err(_) => return (None, None),
    };
    let Some(sess) = sess else {
        return (None, None);
    };

    if sess.revoked_at.is_some() {
        return (None, None);
    }
    if sess.expires_at <= chrono::Utc::now() {
        return (None, None);
    }

    (Some(sess.user_id), Some(sess.id))
}

const COOKIE_SESSION: &str = "rim_session";
const COOKIE_CSRF: &str = "rim_csrf";

fn apply_auth_cookies(
    gql_resp: GraphQLResponse,
    intent_ctx: &AuthIntentCtx,
    req: &HttpRequest,
) -> GraphQLResponse {
    let secure = req
        .headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("https"))
        .unwrap_or(false);

    let mut resp = gql_resp;
    let Some(intent) = intent_ctx.take() else {
        return resp;
    };

    match intent {
        AuthIntent::SetSession {
            session_token,
            csrf_token,
        } => {
            let c1 = actix_web::cookie::Cookie::build(COOKIE_SESSION, session_token)
                .path("/")
                .http_only(true)
                .same_site(actix_web::cookie::SameSite::Lax)
                .secure(secure)
                .finish();

            let c2 = actix_web::cookie::Cookie::build(COOKIE_CSRF, csrf_token)
                .path("/")
                .http_only(false)
                .same_site(actix_web::cookie::SameSite::Lax)
                .secure(secure)
                .finish();

            resp.0 = append_set_cookie(resp.0, c1.to_string());
            resp.0 = append_set_cookie(resp.0, c2.to_string());
        }
        AuthIntent::ClearSession => {
            let mut c1 = actix_web::cookie::Cookie::build(COOKIE_SESSION, "")
                .path("/")
                .http_only(true)
                .same_site(actix_web::cookie::SameSite::Lax)
                .secure(secure)
                .finish();
            c1.make_removal();

            let mut c2 = actix_web::cookie::Cookie::build(COOKIE_CSRF, "")
                .path("/")
                .http_only(false)
                .same_site(actix_web::cookie::SameSite::Lax)
                .secure(secure)
                .finish();
            c2.make_removal();

            resp.0 = append_set_cookie(resp.0, c1.to_string());
            resp.0 = append_set_cookie(resp.0, c2.to_string());
        }
    }

    resp
}

fn append_set_cookie(
    br: async_graphql::BatchResponse,
    cookie: String,
) -> async_graphql::BatchResponse {
    let name = http::header::HeaderName::from_static("set-cookie");
    let value = http::header::HeaderValue::from_str(&cookie).unwrap();

    match br {
        async_graphql::BatchResponse::Single(mut r) => {
            r.http_headers.append(name, value);
            async_graphql::BatchResponse::Single(r)
        }
        async_graphql::BatchResponse::Batch(mut rs) => {
            for r in &mut rs {
                r.http_headers.append(name.clone(), value.clone());
            }
            async_graphql::BatchResponse::Batch(rs)
        }
    }
}
