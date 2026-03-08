use crate::config::RIMConfig;
use actix_web::{App, HttpRequest, HttpServer, guard, web};
use async_graphql_actix_web::{GraphQLRequest, GraphQLResponse};
use log::{debug, info};
use rim_api::AppSchema;
use rim_api::auth_intent::{AuthIntent, AuthIntentCtx, insert_auth_intent_ctx};
use rim_api::build_schema;
use rim_api::graphql::index_graphiql;
use sea_orm::Database;
use tracing::{Instrument, info_span};

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
use rim_infra_services::conversation::DefaultConversationService;
use rim_infra_services::message::DefaultMessageService;
use rim_infra_services::thread::DefaultThreadService;
use rim_services::auth::service::AuthService;
use rim_services::conversation::service::ConversationService;
use rim_services::message::service::MessageService;
use rim_services::message_thread::service::ThreadService;

use rim_api::query::AuthActorCtx;
use uuid::Uuid;

pub async fn start_server(config: &RIMConfig) -> std::io::Result<()> {
    info!("Starting RIM server...");

    debug!("Configuration: {:?}", config);
    let bind_host = format!("{}:{}", config.server_host, config.server_port);

    let db = Database::connect(&config.database_url)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

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

    let conversation_service: Arc<dyn ConversationService> =
        Arc::new(DefaultConversationService::new(repos.for_conversation()));
    let message_service: Arc<dyn MessageService> =
        Arc::new(DefaultMessageService::new(repos.for_message()));
    let thread_service: Arc<dyn ThreadService> =
        Arc::new(DefaultThreadService::new(repos.for_thread()));

    let schema: AppSchema = build_schema(
        Arc::clone(&auth_service),
        Arc::clone(&conversation_service),
        Arc::clone(&message_service),
        Arc::clone(&thread_service),
    );

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
        let session_repo = req
            .app_data::<web::Data<Arc<dyn rim_core::auth::repository::SessionRepository>>>()
            .map(|d| Arc::clone(d.get_ref()));

        let (actor, session_id) = match session_repo {
            Some(repo) => match extract_actor_and_session_id(&req, repo.as_ref()).await {
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

#[allow(clippy::result_large_err)]
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
) -> Result<
    (
        Option<rim_core::user::model::UserId>,
        Option<rim_core::auth::model::SessionId>,
    ),
    rim_core::repository::error::RepoError,
> {
    let cookie = match req.cookie(COOKIE_SESSION) {
        Some(c) => c,
        None => return Ok((None, None)),
    };

    let token_hash = hash_token(cookie.value());
    let sess = session_repo.get_by_token_hash(&token_hash).await?;
    let Some(sess) = sess else {
        return Ok((None, None));
    };

    if sess.revoked_at.is_some() {
        return Ok((None, None));
    }
    if sess.expires_at <= chrono::Utc::now() {
        return Ok((None, None));
    }

    Ok((Some(sess.user_id), Some(sess.id)))
}

fn internal_graphql_error_response(message: &str) -> GraphQLResponse {
    let mut response =
        async_graphql::Response::from_errors(vec![async_graphql::ServerError::new(message, None)]);
    for error in &mut response.errors {
        let ext = error.extensions.get_or_insert_with(Default::default);
        ext.set("code", "INTERNAL");
    }
    GraphQLResponse::from(response)
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

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::test::TestRequest;
    use async_trait::async_trait;
    use rim_core::auth::model::{NewSession, Session, SessionId};
    use rim_core::auth::repository::SessionRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_core::user::model::UserId;
    use std::sync::Mutex;

    struct FakeSessionRepo {
        session: Mutex<Option<RepoResult<Option<Session>>>>,
    }

    impl FakeSessionRepo {
        fn new(session: RepoResult<Option<Session>>) -> Self {
            Self {
                session: Mutex::new(Some(session)),
            }
        }
    }

    #[async_trait]
    impl SessionRepository for FakeSessionRepo {
        async fn get_by_id(&self, _id: SessionId) -> RepoResult<Option<Session>> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn get_by_token_hash(&self, _token_hash: &str) -> RepoResult<Option<Session>> {
            self.session
                .lock()
                .unwrap()
                .take()
                .expect("fake session response not configured")
        }

        async fn create_session(&self, _session: NewSession) -> RepoResult<Session> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn touch_last_seen_at(&self, _id: SessionId) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn revoke_session(&self, _id: SessionId) -> RepoResult<()> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }

        async fn revoke_all_for_user(&self, _user_id: UserId) -> RepoResult<u64> {
            Err(RepoError::new(RepoErrorKind::Unsupported))
        }
    }

    fn session_cookie_request(token: &str) -> HttpRequest {
        TestRequest::default()
            .cookie(actix_web::cookie::Cookie::new(
                COOKIE_SESSION,
                token.to_string(),
            ))
            .to_http_request()
    }

    fn active_session() -> Session {
        Session {
            id: SessionId::new(),
            user_id: UserId::new(),
            token_hash: hash_token("session-token"),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            revoked_at: None,
            created_at: chrono::Utc::now(),
            last_seen_at: Some(chrono::Utc::now()),
            user_agent: None,
            ip: None,
        }
    }

    #[actix_web::test]
    async fn extract_actor_and_session_id_returns_error_on_repo_failure() {
        let req = session_cookie_request("session-token");
        let repo = FakeSessionRepo::new(Err(RepoError::with_message(
            RepoErrorKind::Unavailable,
            "db unavailable",
        )));

        let result = extract_actor_and_session_id(&req, &repo).await;

        assert!(matches!(result, Err(err) if matches!(err.kind(), RepoErrorKind::Unavailable)));
    }

    #[actix_web::test]
    async fn extract_actor_and_session_id_returns_anonymous_for_expired_session() {
        let req = session_cookie_request("session-token");
        let mut session = active_session();
        session.expires_at = chrono::Utc::now() - chrono::Duration::minutes(1);
        let repo = FakeSessionRepo::new(Ok(Some(session)));

        let result = extract_actor_and_session_id(&req, &repo).await.unwrap();

        assert_eq!(result, (None, None));
    }

    #[test]
    fn internal_graphql_error_response_sets_internal_code() {
        let resp = internal_graphql_error_response("failed to validate session");
        let async_graphql::BatchResponse::Single(resp) = resp.0 else {
            panic!("expected single response");
        };

        assert_eq!(resp.errors.len(), 1);
        assert_eq!(resp.errors[0].message, "failed to validate session");
        assert_eq!(
            resp.errors[0]
                .extensions
                .as_ref()
                .and_then(|ext| ext.get("code")),
            Some(&async_graphql::Value::from("INTERNAL"))
        );
    }
}
