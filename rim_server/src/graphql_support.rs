use actix_web::HttpRequest;
use async_graphql_actix_web::GraphQLResponse;
use rim_services::auth::service::RequestAuthenticator;

use rim_api::auth_intent::{AuthIntent, AuthIntentCtx};

pub const COOKIE_SESSION: &str = "rim_session";
pub const COOKIE_CSRF: &str = "rim_csrf";

#[allow(clippy::result_large_err)]
pub fn enforce_csrf(
    req: &HttpRequest,
    gql: &async_graphql::Request,
) -> Result<(), GraphQLResponse> {
    const ALLOW: &[&str] = &["bootstrap", "login", "accept_invite", "logout"];

    let doc = match async_graphql_parser::parse_query(gql.query.as_str()) {
        Ok(d) => d,
        Err(_) => return Ok(()),
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

pub async fn extract_actor_and_session_id(
    req: &HttpRequest,
    request_authenticator: &dyn RequestAuthenticator,
) -> rim_services::ServiceResult<(
    Option<rim_core::user::model::UserId>,
    Option<rim_core::auth::model::SessionId>,
)> {
    let session_token = req
        .cookie(COOKIE_SESSION)
        .map(|cookie| cookie.value().to_string());
    let result = request_authenticator
        .authenticate_session_token(session_token.as_deref())
        .await?;

    Ok((result.user_id, result.session_id))
}

pub fn internal_graphql_error_response(message: &str) -> GraphQLResponse {
    let mut response =
        async_graphql::Response::from_errors(vec![async_graphql::ServerError::new(message, None)]);
    for error in &mut response.errors {
        let ext = error.extensions.get_or_insert_with(Default::default);
        ext.set("code", "INTERNAL");
    }
    GraphQLResponse::from(response)
}

pub fn apply_auth_cookies(
    gql_resp: GraphQLResponse,
    intent_ctx: &AuthIntentCtx,
    req: &HttpRequest,
    session_ttl_days: u32,
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
                .max_age(actix_web::cookie::time::Duration::days(
                    session_ttl_days as i64,
                ))
                .finish();

            let c2 = actix_web::cookie::Cookie::build(COOKIE_CSRF, csrf_token)
                .path("/")
                .http_only(false)
                .same_site(actix_web::cookie::SameSite::Lax)
                .secure(secure)
                .max_age(actix_web::cookie::time::Duration::days(
                    session_ttl_days as i64,
                ))
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
    let value = match http::header::HeaderValue::from_str(&cookie) {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(error = %err, "failed to encode cookie as header value");
            return br;
        }
    };

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
    use rim_core::auth::model::SessionId;
    use rim_core::user::model::UserId;
    use rim_services::auth::dto::RequestAuthResultDto;
    use rim_services::auth::service::MockRequestAuthenticator;

    fn session_cookie_request(token: &str) -> HttpRequest {
        TestRequest::default()
            .cookie(actix_web::cookie::Cookie::new(
                COOKIE_SESSION,
                token.to_string(),
            ))
            .to_http_request()
    }

    #[actix_web::test]
    async fn extract_actor_and_session_id_returns_error_on_repo_failure() {
        let req = session_cookie_request("session-token");
        let mut auth = MockRequestAuthenticator::new();
        auth.expect_authenticate_session_token()
            .withf(|token| *token == Some("session-token"))
            .return_once(|_| Box::pin(async { Err(rim_services::ServiceError::Unavailable) }));

        let result = extract_actor_and_session_id(&req, &auth).await;

        assert!(matches!(
            result,
            Err(rim_services::ServiceError::Unavailable)
        ));
    }

    #[actix_web::test]
    async fn extract_actor_and_session_id_returns_anonymous_for_expired_session() {
        let req = session_cookie_request("session-token");
        let mut auth = MockRequestAuthenticator::new();
        auth.expect_authenticate_session_token()
            .withf(|token| *token == Some("session-token"))
            .return_once(|_| Box::pin(async { Ok(RequestAuthResultDto::default()) }));

        let result = extract_actor_and_session_id(&req, &auth).await.unwrap();

        assert_eq!(result, (None, None));
    }

    #[actix_web::test]
    async fn extract_actor_and_session_id_returns_authenticated_actor() {
        let req = session_cookie_request("session-token");
        let user_id = UserId::new();
        let session_id = SessionId::new();
        let mut auth = MockRequestAuthenticator::new();
        auth.expect_authenticate_session_token()
            .withf(|token| *token == Some("session-token"))
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(RequestAuthResultDto {
                        user_id: Some(user_id),
                        session_id: Some(session_id),
                    })
                })
            });

        let result = extract_actor_and_session_id(&req, &auth).await.unwrap();

        assert_eq!(result, (Some(user_id), Some(session_id)));
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
