use actix_web::HttpRequest;
use async_graphql_actix_web::GraphQLResponse;
use async_graphql_parser::types::{ExecutableDocument, OperationType, Selection};
use rim_auth::csrf::verify_csrf_token;
use rim_services::auth::service::RequestAuthenticator;

use rim_api::auth_intent::{AuthIntent, AuthIntentCtx};

pub const COOKIE_SESSION: &str = "rim_session";
pub const COOKIE_CSRF: &str = "rim_csrf";

/// Extract the actual mutation field names from a parsed GraphQL document.
///
/// This inspects the AST rather than trusting the client-supplied `operationName`,
/// preventing CSRF bypass via operationName manipulation.
fn extract_mutation_fields(doc: &ExecutableDocument) -> Vec<String> {
    let mut fields = Vec::new();
    for (_name, operation) in doc.operations.iter() {
        if matches!(operation.node.ty, OperationType::Mutation) {
            for selection in &operation.node.selection_set.node.items {
                match &selection.node {
                    Selection::Field(field) => {
                        fields.push(field.node.name.node.to_string());
                    }
                    Selection::InlineFragment(inline) => {
                        for sub_selection in &inline.node.selection_set.node.items {
                            if let Selection::Field(field) = &sub_selection.node {
                                fields.push(field.node.name.node.to_string());
                            }
                        }
                    }
                    Selection::FragmentSpread(spread) => {
                        let frag_name = &spread.node.fragment_name.node;
                        if let Some(frag) = doc.fragments.get(frag_name.as_str()) {
                            for sub_selection in &frag.node.selection_set.node.items {
                                if let Selection::Field(field) = &sub_selection.node {
                                    fields.push(field.node.name.node.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    fields
}

/// Mutation field names (camelCase, matching the GraphQL schema) that are
/// exempt from CSRF validation because they are used before the client has
/// a CSRF token.
const CSRF_EXEMPT_MUTATIONS: &[&str] = &["bootstrap", "login", "acceptInvite", "logout"];

/// Enforce CSRF for a batch request. Checks once at the HTTP level whether any
/// request in the batch contains non-exempt mutations.
#[allow(clippy::result_large_err)]
pub fn enforce_csrf_for_batch(
    req: &HttpRequest,
    batch: &async_graphql::BatchRequest,
) -> Result<(), GraphQLResponse> {
    let needs_csrf = batch.iter().any(request_needs_csrf);
    if needs_csrf {
        validate_csrf_tokens(req)
    } else {
        Ok(())
    }
}

/// Returns true if a single GraphQL request contains mutations that require
/// CSRF validation.
fn request_needs_csrf(gql: &async_graphql::Request) -> bool {
    let doc = match async_graphql_parser::parse_query(gql.query.as_str()) {
        Ok(d) => d,
        Err(_) => {
            // Fail-closed: if we can't parse the query, require CSRF.
            return true;
        }
    };

    let has_mutation_ops = doc
        .operations
        .iter()
        .any(|(_, op)| matches!(op.node.ty, OperationType::Mutation));

    if !has_mutation_ops {
        return false;
    }

    let mutation_fields = extract_mutation_fields(&doc);

    // Fail-closed: if mutation operations exist but we couldn't extract any
    // field names (e.g. deeply nested fragment spreads we don't resolve),
    // require CSRF rather than silently skipping validation.
    if mutation_fields.is_empty() {
        return true;
    }

    // All mutation fields must be in the exempt list to skip CSRF.
    let all_exempt = mutation_fields.iter().all(|f| {
        CSRF_EXEMPT_MUTATIONS
            .iter()
            .any(|a| a.eq_ignore_ascii_case(f))
    });

    !all_exempt
}

/// Validate CSRF token using the double-submit cookie pattern with HMAC session binding.
///
/// 1. The `X-CSRF-Token` header must match the `rim_csrf` cookie (double-submit).
/// 2. The CSRF token must be a valid HMAC signature over the session token from
///    the `rim_session` cookie, binding the token to the originating session.
pub fn validate_csrf_header(req: &HttpRequest) -> Result<(), &'static str> {
    let csrf_cookie = req.cookie(COOKIE_CSRF).map(|c| c.value().to_string());
    let csrf_header = req
        .headers()
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Step 1: Double-submit check — cookie must match header.
    let csrf_token = match (csrf_cookie, csrf_header) {
        (Some(c), Some(h)) if c == h => c,
        _ => return Err("csrf validation failed"),
    };

    // Step 2: HMAC session binding — token must be signed with the session token.
    let session_token = req
        .cookie(COOKIE_SESSION)
        .map(|c| c.value().to_string())
        .ok_or("csrf validation failed: missing session")?;

    if !verify_csrf_token(&csrf_token, &session_token) {
        return Err("csrf validation failed: token not bound to session");
    }

    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_csrf_tokens(req: &HttpRequest) -> Result<(), GraphQLResponse> {
    validate_csrf_header(req).map_err(|msg| {
        GraphQLResponse::from(async_graphql::Response::from_errors(vec![
            async_graphql::ServerError::new(msg, None),
        ]))
    })
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
