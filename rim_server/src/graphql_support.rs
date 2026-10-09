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
const CSRF_EXEMPT_MUTATIONS: &[&str] = &[
    "login",
    "logout",
    // signupWithInvite is unauthenticated (no session, no CSRF token yet).
    "signupWithInvite",
    // Password reset is used signed out, from the emailed link.
    "requestPasswordReset",
    "resetPassword",
    // Key login and key-only signup are used before there is a session.
    "keyLoginChallenge",
    "loginWithKey",
    "signupWithKey",
];

/// Mutation fields exempt from CSRF validation only when the request carries no session cookie.
///
/// `rotateIdentityKey` is how a person who lost every device recovers, signed out. An identity
/// rotation, though, is accepted only on the strength of the caller's session, so a request that
/// carries one needs the CSRF header like any other mutation; otherwise a forged same-site
/// request could ride the session.
const CSRF_EXEMPT_WITHOUT_SESSION: &[&str] = &["rotateIdentityKey"];

/// Enforce CSRF for a batch request. Checks once at the HTTP level whether any
/// request in the batch contains non-exempt mutations.
#[allow(clippy::result_large_err)]
pub fn enforce_csrf_for_batch(
    req: &HttpRequest,
    batch: &async_graphql::BatchRequest,
) -> Result<(), GraphQLResponse> {
    let has_session = req.cookie(COOKIE_SESSION).is_some();
    let needs_csrf = batch.iter().any(|gql| request_needs_csrf(gql, has_session));
    if needs_csrf {
        validate_csrf_tokens(req)
    } else {
        Ok(())
    }
}

/// Returns true if a single GraphQL request contains mutations that require
/// CSRF validation. `has_session` says whether the request carries a session cookie.
fn request_needs_csrf(gql: &async_graphql::Request, has_session: bool) -> bool {
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
            || (!has_session
                && CSRF_EXEMPT_WITHOUT_SESSION
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(f)))
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

/// Determine whether the `Secure` cookie attribute should be set.
///
/// H3 hardening — priority order:
///
/// 1. `force_secure_cookies = true` → always `Secure`. Use for direct-HTTPS
///    deployments (Tauri WebView, Caddy/Traefik without `X-Forwarded-Proto`).
/// 2. `trust_forwarded_headers = true` and actix connection scheme is `"https"`
///    → `Secure`. When a trusted reverse proxy is configured, actix derives the
///    scheme from `X-Forwarded-Proto` or `Forwarded` headers.
/// 3. Otherwise `false` (plain HTTP or no explicit trust configured).
///
/// **Why not read `X-Forwarded-Proto` without trust?** actix-web's
/// `connection_info().scheme()` already reads that header. Gating the entire
/// scheme check on `trust_forwarded_headers` ensures an attacker on a plain-HTTP
/// connection cannot forge the header to influence the `Secure` flag unless the
/// operator has explicitly configured a trusted proxy (H1/H3 alignment).
pub(crate) fn resolve_secure_flag(
    req: &HttpRequest,
    trust_forwarded_headers: bool,
    force_secure_cookies: bool,
) -> bool {
    // Priority 1: operator override — unconditional.
    if force_secure_cookies {
        return true;
    }
    // Priority 2: scheme-based detection, only when a trusted proxy is configured.
    // actix derives the scheme from X-Forwarded-Proto / Forwarded when present,
    // so we must gate this on trust to prevent header spoofing on bare HTTP.
    if trust_forwarded_headers && req.connection_info().scheme() == "https" {
        return true;
    }
    false
}

/// Apply session / CSRF cookies from the resolved [`AuthIntent`] onto the response.
///
/// Uses [`resolve_secure_flag`] to determine the `Secure` attribute so that the
/// flag is set correctly for both direct-TLS and proxied deployments (H3 hardening).
pub fn apply_auth_cookies(
    gql_resp: GraphQLResponse,
    intent_ctx: &AuthIntentCtx,
    req: &HttpRequest,
    session_ttl_days: u32,
    trust_forwarded_headers: bool,
    force_secure_cookies: bool,
) -> GraphQLResponse {
    let secure = resolve_secure_flag(req, trust_forwarded_headers, force_secure_cookies);

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

    #[test]
    fn signed_out_mutations_skip_csrf() {
        for query in [
            "mutation { keyLoginChallenge { nonce } }",
            r#"mutation { loginWithKey(proof: { payload: "p", signature: "s" }) { userId } }"#,
            r#"mutation { signupWithKey(input: { username: "u", keyLog: [], device: { payload: "p", signature: "s" }, proof: { payload: "p", signature: "s" } }) { userId } }"#,
            r#"mutation { rotateIdentityKey(input: { entry: { payload: "p", signature: "s" } }) { keySeq } }"#,
            r#"mutation { requestPasswordReset(username: "u") }"#,
            r#"mutation { resetPassword(token: "t", newPassword: "p") }"#,
        ] {
            assert!(
                !request_needs_csrf(&async_graphql::Request::new(query), false),
                "{query}"
            );
        }
    }

    #[test]
    fn rotation_with_a_session_needs_csrf() {
        let rotate = async_graphql::Request::new(
            r#"mutation { rotateIdentityKey(input: { entry: { payload: "p", signature: "s" } }) { keySeq } }"#,
        );
        assert!(request_needs_csrf(&rotate, true));
        // Login stays exempt with a stale session cookie, so signing in again always works.
        let login = async_graphql::Request::new(
            r#"mutation { loginWithKey(proof: { payload: "p", signature: "s" }) { userId } }"#,
        );
        assert!(!request_needs_csrf(&login, true));
    }

    #[test]
    fn batch_rotation_with_session_cookie_and_no_csrf_header_is_refused() {
        let batch = async_graphql::BatchRequest::Single(async_graphql::Request::new(
            r#"mutation { rotateIdentityKey(input: { entry: { payload: "p", signature: "s" } }) { keySeq } }"#,
        ));
        let signed_in = TestRequest::default()
            .cookie(actix_web::cookie::Cookie::new(COOKIE_SESSION, "rim_sess_x"))
            .to_http_request();
        assert!(enforce_csrf_for_batch(&signed_in, &batch).is_err());
        let signed_out = TestRequest::default().to_http_request();
        assert!(enforce_csrf_for_batch(&signed_out, &batch).is_ok());
    }

    #[test]
    fn key_mutations_with_a_session_need_csrf() {
        for query in [
            r#"mutation { revokeDeviceKey(revocation: { payload: "p", signature: "s" }) { name } }"#,
            r#"mutation { loginWithKey(proof: { payload: "p", signature: "s" }) { userId } addDeviceKey(certificate: { payload: "p", signature: "s" }) { name } }"#,
        ] {
            assert!(
                request_needs_csrf(&async_graphql::Request::new(query), false),
                "{query}"
            );
        }
    }

    // ── H1 / H3 tests ───────────────────────────────────────────────────────

    /// H3 — `force_secure_cookies = true` always returns `true`, regardless of
    /// any headers or scheme.
    #[test]
    fn resolve_secure_flag_force_override_always_true() {
        let req = TestRequest::default().to_http_request();
        assert!(resolve_secure_flag(&req, false, true));
        assert!(resolve_secure_flag(&req, true, true));
    }

    /// H3 — Plain HTTP connection without force or X-Forwarded-Proto returns `false`.
    #[test]
    fn resolve_secure_flag_plain_http_no_trust_returns_false() {
        let req = TestRequest::default().to_http_request();
        assert!(!resolve_secure_flag(&req, false, false));
    }

    /// H1/H3 — X-Forwarded-Proto header is ignored when trust is disabled.
    #[test]
    fn resolve_secure_flag_xff_proto_ignored_without_trust() {
        let req = TestRequest::default()
            .insert_header(("x-forwarded-proto", "https"))
            .to_http_request();
        // Even though the header says https, trust=false means we ignore it.
        assert!(!resolve_secure_flag(&req, false, false));
    }

    /// H1/H3 — X-Forwarded-Proto header is honoured when trust is enabled.
    #[test]
    fn resolve_secure_flag_xff_proto_honoured_with_trust() {
        let req = TestRequest::default()
            .insert_header(("x-forwarded-proto", "https"))
            .to_http_request();
        assert!(resolve_secure_flag(&req, true, false));
    }

    /// H1/H3 — X-Forwarded-Proto: http with trust=true returns false.
    #[test]
    fn resolve_secure_flag_xff_proto_http_with_trust_returns_false() {
        let req = TestRequest::default()
            .insert_header(("x-forwarded-proto", "http"))
            .to_http_request();
        assert!(!resolve_secure_flag(&req, true, false));
    }
}
