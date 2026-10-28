use actix_web::HttpRequest;
use async_graphql_actix_web::GraphQLResponse;
use rim_auth::csrf::verify_csrf_token;
use rim_services::auth::service::RequestAuthenticator;

use rim_api::auth_intent::{AuthIntent, AuthIntentCtx};
use rim_api::operation::{OperationFields, root_fields};
use rim_services::auth::dto::SessionSecret;

pub const COOKIE_SESSION: &str = "rim_session";
pub const COOKIE_CSRF: &str = "rim_csrf";
/// The header that carries an unkept session's secret.
pub const SESSION_SECRET_HEADER: &str = "x-rim-session-secret";

/// Mutation fields an unkept session runs without its secret: the tab that reloaded and lost
/// the secret takes its reload key, or signs out.
const SECRET_EXEMPT_MUTATIONS: &[&str] = &["reloadSession", "logout"];

/// The root fields of one request of a batch, or the message its document fails to parse with.
pub type RootFields = Result<OperationFields, String>;

/// The root fields of every request of `batch`, in order. Each document is parsed once and kept
/// in its request, so running the batch does not parse it again.
pub fn batch_root_fields(batch: &mut async_graphql::BatchRequest) -> Vec<RootFields> {
    batch
        .iter_mut()
        .map(|request| {
            let operation_name = request.operation_name.clone();
            request
                .parsed_query()
                .map(|document| root_fields(document, operation_name.as_deref()))
                .map_err(|e| e.message)
        })
        .collect()
}

/// Whether a request of the batch defines a mutation operation, selected or not.
pub fn batch_has_mutation(fields: &[RootFields]) -> bool {
    fields
        .iter()
        .any(|f| f.as_ref().is_ok_and(|f| f.mutation_in_document))
}

/// Mutation fields across a batch, aliases, repeats and each fragment expansion counted; a
/// request that does not parse counts none, since it runs nothing.
pub fn count_mutation_fields(fields: &[RootFields]) -> u32 {
    let count: usize = fields
        .iter()
        .filter_map(|f| f.as_ref().ok())
        .map(OperationFields::mutation_count)
        .sum();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Whether every request of the batch is a mutation whose root fields, all found, are among
/// [`SECRET_EXEMPT_MUTATIONS`]: then an unkept session's cookie counts without its secret.
pub fn batch_needs_no_secret(fields: &[RootFields]) -> bool {
    !fields.is_empty()
        && fields.iter().all(|f| {
            f.as_ref().is_ok_and(|f| {
                f.operation == Some(async_graphql::parser::types::OperationType::Mutation)
                    && f.is_complete()
                    && !f.fields.is_empty()
                    && f.fields
                        .iter()
                        .all(|name| SECRET_EXEMPT_MUTATIONS.contains(&name.as_str()))
            })
        })
}

/// Mutation field names (camelCase, matching the GraphQL schema) that are
/// exempt from CSRF validation because they are used before the client has
/// a CSRF token.
const CSRF_EXEMPT_MUTATIONS: &[&str] = &[
    "logout",
    // Key login and key signup are used before there is a session.
    "keyLoginChallenge",
    "loginWithKey",
    "signupWithKey",
    // The password steps around key login and signup, and the reset of a forgotten password,
    // run before there is a session.
    "passwordRegistrationStart",
    "passwordLoginStart",
    "passwordLoginFinish",
    "passwordResetChallenge",
    "passwordResetStart",
    "passwordResetFinish",
    // The new device sends its sealed offer before it has a session. It acts only on the link
    // id, a capability a cross-site page does not have.
    "sendDeviceLinkOffer",
    // A device revocation brought to a host directly. It acts only on the revocation's
    // signature, never on a session.
    "applyRemoteRevocation",
];

/// Mutation fields exempt from CSRF validation only when the request carries no session cookie.
///
/// `rotateIdentityKey` is how a person who lost every device recovers, signed out. An identity
/// rotation, though, is accepted only on the strength of the caller's session, so a request that
/// carries one needs the CSRF header like any other mutation; otherwise a forged same-site
/// request could ride the session.
///
/// `cancelDeviceLink` is how a new device without a session abandons a link. With a session
/// cookie it may act as the link's user, so it needs the header then.
const CSRF_EXEMPT_WITHOUT_SESSION: &[&str] = &["rotateIdentityKey", "cancelDeviceLink"];

/// Enforce CSRF for a batch request. Checks once at the HTTP level whether any
/// request in the batch contains non-exempt mutations.
#[allow(clippy::result_large_err)]
pub fn enforce_csrf_for_batch(
    req: &HttpRequest,
    fields: &[RootFields],
) -> Result<(), GraphQLResponse> {
    let has_session = req.cookie(COOKIE_SESSION).is_some();
    let needs_csrf = fields.iter().any(|f| request_needs_csrf(f, has_session));
    if needs_csrf {
        validate_csrf_tokens(req)
    } else {
        Ok(())
    }
}

/// Whether a single GraphQL request needs the CSRF header: it may run a mutation and either its
/// root fields cannot all be found, through fragments at any depth, or one is not exempt. A
/// request that does not parse needs it. `has_session` says whether the request carries a
/// session cookie.
fn request_needs_csrf(fields: &RootFields, has_session: bool) -> bool {
    let Ok(fields) = fields else {
        return true;
    };
    if !fields.may_mutate() {
        return false;
    }
    if !fields.is_complete() || fields.fields.is_empty() {
        return true;
    }
    let all_exempt = fields.fields.iter().all(|f| {
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

/// The session cookie's user, session and the device the session is bound to. An unkept
/// session counts only when the request carries its secret in [`SESSION_SECRET_HEADER`];
/// otherwise the request is anonymous.
pub async fn extract_auth(
    req: &HttpRequest,
    request_authenticator: &dyn RequestAuthenticator,
) -> rim_services::ServiceResult<rim_services::auth::dto::RequestAuthResultDto> {
    let secret = req
        .headers()
        .get(SESSION_SECRET_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    authenticate(req, request_authenticator, SessionSecret::Presented(secret)).await
}

/// [`extract_auth`] without the session secret, for the requests an unkept session makes
/// without it: plain file downloads, the WebSocket upgrade (whose `connection_init` carries
/// the secret), `reloadSession` and `logout`.
pub async fn extract_auth_without_secret(
    req: &HttpRequest,
    request_authenticator: &dyn RequestAuthenticator,
) -> rim_services::ServiceResult<rim_services::auth::dto::RequestAuthResultDto> {
    authenticate(req, request_authenticator, SessionSecret::NotRequired).await
}

async fn authenticate(
    req: &HttpRequest,
    request_authenticator: &dyn RequestAuthenticator,
    secret: SessionSecret,
) -> rim_services::ServiceResult<rim_services::auth::dto::RequestAuthResultDto> {
    let session_token = req
        .cookie(COOKIE_SESSION)
        .map(|cookie| cookie.value().to_string());
    request_authenticator
        .authenticate_session_token(session_token.as_deref(), secret)
        .await
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
/// Priority order:
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
/// operator has explicitly configured a trusted proxy.
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
    // so this is gated on trust to prevent header spoofing on bare HTTP.
    if trust_forwarded_headers && req.connection_info().scheme() == "https" {
        return true;
    }
    false
}

/// The `SameSite` attribute of the session and CSRF cookies for `req`.
///
/// `None` when the request's `Origin` is one of the packaged desktop app's and the cookies are
/// `Secure`: the desktop app's page is on another site than the server, and a browser keeps and
/// sends a `Lax` cookie on such a request only for top-level navigations. `Lax` otherwise,
/// including over plain http, where a browser refuses `SameSite=None` without `Secure`.
fn session_same_site(req: &HttpRequest, secure: bool) -> actix_web::cookie::SameSite {
    let desktop = req
        .headers()
        .get(actix_web::http::header::ORIGIN)
        .and_then(|origin| origin.to_str().ok())
        .is_some_and(crate::origin::is_desktop_origin);
    if desktop && secure {
        actix_web::cookie::SameSite::None
    } else {
        actix_web::cookie::SameSite::Lax
    }
}

/// Apply session / CSRF cookies from the resolved [`AuthIntent`] onto the response.
///
/// Uses [`resolve_secure_flag`] to determine the `Secure` attribute so that the
/// flag is set correctly for both direct-TLS and proxied deployments, and
/// [`session_same_site`] for the `SameSite` attribute. A kept session's cookies
/// last `session_ttl_days`; an unkept session's have no `Max-Age`, so the browser drops them
/// when it closes.
pub fn apply_auth_cookies(
    gql_resp: GraphQLResponse,
    intent_ctx: &AuthIntentCtx,
    req: &HttpRequest,
    session_ttl_days: u32,
    trust_forwarded_headers: bool,
    force_secure_cookies: bool,
) -> GraphQLResponse {
    let secure = resolve_secure_flag(req, trust_forwarded_headers, force_secure_cookies);
    let same_site = session_same_site(req, secure);

    let mut resp = gql_resp;
    let Some(intent) = intent_ctx.take() else {
        return resp;
    };

    match intent {
        AuthIntent::SetSession {
            session_token,
            csrf_token,
            kept,
        } => {
            let max_age =
                kept.then(|| actix_web::cookie::time::Duration::days(session_ttl_days as i64));
            let mut c1 = actix_web::cookie::Cookie::build(COOKIE_SESSION, session_token)
                .path("/")
                .http_only(true)
                .same_site(same_site)
                .secure(secure)
                .finish();

            let mut c2 = actix_web::cookie::Cookie::build(COOKIE_CSRF, csrf_token)
                .path("/")
                .http_only(false)
                .same_site(same_site)
                .secure(secure)
                .finish();
            if let Some(max_age) = max_age {
                c1.set_max_age(max_age);
                c2.set_max_age(max_age);
            }

            resp.0 = append_set_cookie(resp.0, c1.to_string());
            resp.0 = append_set_cookie(resp.0, c2.to_string());
        }
        AuthIntent::ClearSession => {
            let mut c1 = actix_web::cookie::Cookie::build(COOKIE_SESSION, "")
                .path("/")
                .http_only(true)
                .same_site(same_site)
                .secure(secure)
                .finish();
            c1.make_removal();

            let mut c2 = actix_web::cookie::Cookie::build(COOKIE_CSRF, "")
                .path("/")
                .http_only(false)
                .same_site(same_site)
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

    /// The root fields of `request`, as [`batch_root_fields`] finds them.
    fn fields_of(request: &async_graphql::Request) -> RootFields {
        let mut copy = async_graphql::Request::new(request.query.clone());
        if let Some(name) = &request.operation_name {
            copy = copy.operation_name(name.clone());
        }
        batch_root_fields(&mut async_graphql::BatchRequest::Single(copy)).remove(0)
    }

    /// The root fields of every request of `batch`.
    fn batch_fields(batch: &async_graphql::BatchRequest) -> Vec<RootFields> {
        batch.iter().map(fields_of).collect()
    }

    fn session_cookie_request(token: &str) -> HttpRequest {
        TestRequest::default()
            .cookie(actix_web::cookie::Cookie::new(
                COOKIE_SESSION,
                token.to_string(),
            ))
            .to_http_request()
    }

    #[test]
    fn each_document_is_parsed_once_and_kept_for_execution() {
        let mut batch = async_graphql::BatchRequest::Batch(vec![
            async_graphql::Request::new("query Q { me { id } } mutation M { logout }")
                .operation_name("Q"),
            async_graphql::Request::new("mutation {"),
        ]);
        let fields = batch_root_fields(&mut batch);
        let first = fields[0].as_ref().expect("parses");
        assert_eq!(first.fields, vec!["me".to_string()]);
        assert!(first.mutation_in_document);
        assert!(fields[1].is_err());
        assert!(batch_has_mutation(&fields));
        assert!(!batch_has_mutation(&fields[1..]));
        let async_graphql::BatchRequest::Batch(mut requests) = batch else {
            panic!("a batch");
        };
        let mut kept = requests.remove(0);
        kept.query = "not a document".to_string();
        assert!(
            kept.parsed_query().is_ok(),
            "the parsed document is kept in the request"
        );
    }

    #[test]
    fn count_mutation_fields_counts_every_field_across_the_batch() {
        let batch = async_graphql::BatchRequest::Batch(vec![
            async_graphql::Request::new("mutation { a: logout b: logout }"),
            async_graphql::Request::new("{ me { id } }"),
            async_graphql::Request::new("mutation M { ...F } fragment F on Mutation { logout }"),
            async_graphql::Request::new("mutation {"),
        ]);
        assert_eq!(count_mutation_fields(&batch_fields(&batch)), 3);
    }

    /// `depth` inline fragments around `field`.
    fn nested_inline(depth: usize, field: &str) -> String {
        format!(
            "mutation {{ {}{field}{} }}",
            "... on Mutation { ".repeat(depth),
            " }".repeat(depth)
        )
    }

    #[test]
    fn csrf_decision_and_budget_charge_follow_every_fragment() {
        const PROFILE: &str = r#"updateMyProfile(input: { statusText: "s" }) { id }"#;
        let depth = rim_api::operation::MAX_FRAGMENT_DEPTH;
        let several =
            format!("query Q {{ me {{ id }} }} mutation M {{ {PROFILE} }} mutation L {{ logout }}");
        // (case, query, operation name, needs the header, mutation fields charged)
        let cases: Vec<(&str, String, Option<&str>, bool, u32)> = vec![
            (
                "the review's document",
                r#"mutation {
  logout @skip(if: true)
  ...Outer
}
fragment Outer on Mutation { ...Inner }
fragment Inner on Mutation {
  updateMyProfile(input: { statusText: "changed without CSRF" }) { id }
}"#
                .to_string(),
                None,
                true,
                2,
            ),
            (
                "protected field in a nested named fragment beside an exempt one",
                format!(
                    "mutation {{ signupWithKey(input: {{ username: \"u\" }}) {{ userId }} ...A }} fragment A on Mutation {{ ...B }} fragment B on Mutation {{ {PROFILE} }}"
                ),
                None,
                true,
                2,
            ),
            (
                "nested inline fragments",
                format!("mutation {{ logout ... on Mutation {{ ... {{ {PROFILE} }} }} }}"),
                None,
                true,
                2,
            ),
            (
                "aliases of an exempt field",
                "mutation { a: logout b: logout ... on Mutation { c: logout } }".to_string(),
                None,
                false,
                3,
            ),
            (
                "the same fragment spread twice",
                "mutation { ...F ...F } fragment F on Mutation { logout }".to_string(),
                None,
                false,
                2,
            ),
            (
                "a cyclic fragment pair",
                "mutation { logout ...A } fragment A on Mutation { ...B } fragment B on Mutation { ...A }"
                    .to_string(),
                None,
                true,
                1,
            ),
            (
                "an exempt field at the depth bound",
                nested_inline(depth, "logout"),
                None,
                false,
                1,
            ),
            (
                "an exempt field past the depth bound",
                nested_inline(depth + 1, "logout"),
                None,
                true,
                0,
            ),
            (
                "an unknown fragment",
                "mutation { logout ...Missing }".to_string(),
                None,
                true,
                1,
            ),
            ("the exempt operation of several", several.clone(), Some("L"), false, 1),
            ("the protected operation of several", several.clone(), Some("M"), true, 1),
            ("the query of several", several.clone(), Some("Q"), false, 0),
            ("several operations and no name", several.clone(), None, true, 0),
            ("a name no operation has", several, Some("X"), true, 0),
        ];
        for (case, query, operation_name, needs_csrf, charged) in cases {
            let mut request = async_graphql::Request::new(query);
            if let Some(name) = operation_name {
                request = request.operation_name(name);
            }
            assert_eq!(
                request_needs_csrf(&fields_of(&request), true),
                needs_csrf,
                "{case}"
            );
            let batch = async_graphql::BatchRequest::Single(request);
            assert_eq!(
                count_mutation_fields(&batch_fields(&batch)),
                charged,
                "{case}"
            );
            let signed_in = TestRequest::default()
                .cookie(actix_web::cookie::Cookie::new(COOKIE_SESSION, "rim_sess_x"))
                .to_http_request();
            assert_eq!(
                enforce_csrf_for_batch(&signed_in, &batch_fields(&batch)).is_err(),
                needs_csrf,
                "{case}"
            );
        }
    }

    #[actix_web::test]
    async fn extract_auth_returns_error_on_repo_failure() {
        let req = session_cookie_request("session-token");
        let mut auth = MockRequestAuthenticator::new();
        auth.expect_authenticate_session_token()
            .withf(|token, _| *token == Some("session-token"))
            .return_once(|_, _| Box::pin(async { Err(rim_services::ServiceError::Unavailable) }));

        let result = extract_auth(&req, &auth).await;

        assert!(matches!(
            result,
            Err(rim_services::ServiceError::Unavailable)
        ));
    }

    #[actix_web::test]
    async fn extract_auth_returns_anonymous_for_expired_session() {
        let req = session_cookie_request("session-token");
        let mut auth = MockRequestAuthenticator::new();
        auth.expect_authenticate_session_token()
            .withf(|token, _| *token == Some("session-token"))
            .return_once(|_, _| Box::pin(async { Ok(RequestAuthResultDto::default()) }));

        let result = extract_auth(&req, &auth).await.unwrap();

        assert_eq!(result.user_id, None);
        assert_eq!(result.session_id, None);
    }

    #[actix_web::test]
    async fn extract_auth_returns_authenticated_actor() {
        let req = session_cookie_request("session-token");
        let user_id = UserId::new();
        let session_id = SessionId::new();
        let mut auth = MockRequestAuthenticator::new();
        auth.expect_authenticate_session_token()
            .withf(|token, _| *token == Some("session-token"))
            .return_once(move |_, _| {
                Box::pin(async move {
                    Ok(RequestAuthResultDto {
                        user_id: Some(user_id),
                        session_id: Some(session_id),
                        device_key_id: None,
                        expires_at: None,
                        secret_hash: None,
                    })
                })
            });

        let result = extract_auth(&req, &auth).await.unwrap();

        assert_eq!(result.user_id, Some(user_id));
        assert_eq!(result.session_id, Some(session_id));
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
        ] {
            assert!(
                !request_needs_csrf(&fields_of(&async_graphql::Request::new(query)), false),
                "{query}"
            );
        }
    }

    #[test]
    fn rotation_with_a_session_needs_csrf() {
        let rotate = async_graphql::Request::new(
            r#"mutation { rotateIdentityKey(input: { entry: { payload: "p", signature: "s" } }) { keySeq } }"#,
        );
        assert!(request_needs_csrf(&fields_of(&rotate), true));
        // Login stays exempt with a stale session cookie, so signing in again always works.
        let login = async_graphql::Request::new(
            r#"mutation { loginWithKey(proof: { payload: "p", signature: "s" }) { userId } }"#,
        );
        assert!(!request_needs_csrf(&fields_of(&login), true));
    }

    #[test]
    fn batch_rotation_with_session_cookie_and_no_csrf_header_is_refused() {
        let batch = async_graphql::BatchRequest::Single(async_graphql::Request::new(
            r#"mutation { rotateIdentityKey(input: { entry: { payload: "p", signature: "s" } }) { keySeq } }"#,
        ));
        let signed_in = TestRequest::default()
            .cookie(actix_web::cookie::Cookie::new(COOKIE_SESSION, "rim_sess_x"))
            .to_http_request();
        assert!(enforce_csrf_for_batch(&signed_in, &batch_fields(&batch)).is_err());
        let signed_out = TestRequest::default().to_http_request();
        assert!(enforce_csrf_for_batch(&signed_out, &batch_fields(&batch)).is_ok());
    }

    #[test]
    fn key_mutations_with_a_session_need_csrf() {
        for query in [
            r#"mutation { revokeDeviceKey(revocation: { payload: "p", signature: "s" }) { name } }"#,
            r#"mutation { loginWithKey(proof: { payload: "p", signature: "s" }) { userId } addDeviceKey(certificate: { payload: "p", signature: "s" }) { name } }"#,
        ] {
            assert!(
                request_needs_csrf(&fields_of(&async_graphql::Request::new(query)), false),
                "{query}"
            );
        }
    }

    #[test]
    fn device_link_offer_skips_csrf_with_or_without_a_session() {
        let offer = async_graphql::Request::new(
            r#"mutation { sendDeviceLinkOffer(linkId: "l", offer: { nonce: "n", ciphertext: "c" }) }"#,
        );
        assert!(!request_needs_csrf(&fields_of(&offer), false));
        assert!(!request_needs_csrf(&fields_of(&offer), true));
    }

    #[test]
    fn device_link_cancel_skips_csrf_only_without_a_session() {
        let cancel = async_graphql::Request::new(r#"mutation { cancelDeviceLink(linkId: "l") }"#);
        assert!(!request_needs_csrf(&fields_of(&cancel), false));
        assert!(request_needs_csrf(&fields_of(&cancel), true));

        let batch = async_graphql::BatchRequest::Single(cancel);
        let signed_in = TestRequest::default()
            .cookie(actix_web::cookie::Cookie::new(COOKIE_SESSION, "rim_sess_x"))
            .to_http_request();
        assert!(enforce_csrf_for_batch(&signed_in, &batch_fields(&batch)).is_err());
        let signed_out = TestRequest::default().to_http_request();
        assert!(enforce_csrf_for_batch(&signed_out, &batch_fields(&batch)).is_ok());
    }

    #[test]
    fn other_device_link_mutations_need_csrf() {
        for query in [
            "mutation { startDeviceLink { linkId } }",
            r#"mutation { sendDeviceLinkAnswer(linkId: "l", answer: { nonce: "n", ciphertext: "c" }) { state } }"#,
            r#"mutation { completeDeviceLink(linkId: "l") }"#,
            // An exempt mutation does not carry a non-exempt one past the check.
            r#"mutation { sendDeviceLinkOffer(linkId: "l", offer: { nonce: "n", ciphertext: "c" }) completeDeviceLink(linkId: "l") }"#,
        ] {
            let request = async_graphql::Request::new(query);
            assert!(request_needs_csrf(&fields_of(&request), false), "{query}");
            assert!(request_needs_csrf(&fields_of(&request), true), "{query}");
        }
    }

    #[test]
    fn workspace_plugin_mutations_need_csrf() {
        for query in [
            r#"mutation { addWorkspacePlugin(input: { serverId: "s", statement: { payload: "p", signature: "s" }, status: REQUIRED }) { id } }"#,
            r#"mutation { updateWorkspacePlugin(input: { serverId: "s", pluginId: "p", status: RECOMMENDED }) { id } }"#,
            r#"mutation { removeWorkspacePlugin(serverId: "s", pluginId: "p") }"#,
        ] {
            let request = async_graphql::Request::new(query);
            assert!(request_needs_csrf(&fields_of(&request), false), "{query}");
            assert!(request_needs_csrf(&fields_of(&request), true), "{query}");
            let batch = async_graphql::BatchRequest::Single(request);
            let signed_in = TestRequest::default()
                .cookie(actix_web::cookie::Cookie::new(COOKIE_SESSION, "rim_sess_x"))
                .to_http_request();
            assert!(
                enforce_csrf_for_batch(&signed_in, &batch_fields(&batch)).is_err(),
                "{query}"
            );
        }
    }

    // ── Secure cookie flag tests ────────────────────────────────────────────

    /// `force_secure_cookies = true` always returns `true`, regardless of
    /// any headers or scheme.
    #[test]
    fn resolve_secure_flag_force_override_always_true() {
        let req = TestRequest::default().to_http_request();
        assert!(resolve_secure_flag(&req, false, true));
        assert!(resolve_secure_flag(&req, true, true));
    }

    /// Plain HTTP connection without force or X-Forwarded-Proto returns `false`.
    #[test]
    fn resolve_secure_flag_plain_http_no_trust_returns_false() {
        let req = TestRequest::default().to_http_request();
        assert!(!resolve_secure_flag(&req, false, false));
    }

    /// X-Forwarded-Proto header is ignored when trust is disabled.
    #[test]
    fn resolve_secure_flag_xff_proto_ignored_without_trust() {
        let req = TestRequest::default()
            .insert_header(("x-forwarded-proto", "https"))
            .to_http_request();
        // Even though the header says https, trust=false means it is ignored.
        assert!(!resolve_secure_flag(&req, false, false));
    }

    /// X-Forwarded-Proto header is honoured when trust is enabled.
    #[test]
    fn resolve_secure_flag_xff_proto_honoured_with_trust() {
        let req = TestRequest::default()
            .insert_header(("x-forwarded-proto", "https"))
            .to_http_request();
        assert!(resolve_secure_flag(&req, true, false));
    }

    /// X-Forwarded-Proto: http with trust=true returns false.
    #[test]
    fn resolve_secure_flag_xff_proto_http_with_trust_returns_false() {
        let req = TestRequest::default()
            .insert_header(("x-forwarded-proto", "http"))
            .to_http_request();
        assert!(!resolve_secure_flag(&req, true, false));
    }

    /// The `Set-Cookie` values `apply_auth_cookies` adds for a new session that is `kept` or
    /// not, with a 30-day TTL.
    fn session_cookies(kept: bool) -> Vec<String> {
        let intent = AuthIntent::SetSession {
            session_token: "rim_sess_t".to_string(),
            csrf_token: "rim_csrf_t".to_string(),
            kept,
        };
        auth_cookies_for(intent, None, false)
    }

    #[test]
    fn a_kept_sessions_cookies_last_the_ttl_and_an_unkept_sessions_the_browser_session() {
        let kept = session_cookies(true);
        assert_eq!(kept.len(), 2);
        for cookie in &kept {
            assert!(cookie.contains("Max-Age=2592000"), "{cookie}");
        }
        let unkept = session_cookies(false);
        assert_eq!(unkept.len(), 2);
        assert!(unkept[0].starts_with("rim_session=rim_sess_t"));
        assert!(unkept[1].starts_with("rim_csrf=rim_csrf_t"));
        for cookie in &unkept {
            assert!(!cookie.contains("Max-Age"), "{cookie}");
            assert!(!cookie.contains("Expires"), "{cookie}");
        }
    }

    /// The `Set-Cookie` values `apply_auth_cookies` adds for `intent` on a request with the
    /// given `Origin` header, with `Secure` forced or not.
    fn auth_cookies_for(intent: AuthIntent, origin: Option<&str>, secure: bool) -> Vec<String> {
        let ctx = AuthIntentCtx::default();
        ctx.set(intent);
        let response =
            GraphQLResponse::from(async_graphql::Response::new(async_graphql::Value::Null));
        let mut req = TestRequest::default();
        if let Some(origin) = origin {
            req = req.insert_header(("origin", origin));
        }
        let response =
            apply_auth_cookies(response, &ctx, &req.to_http_request(), 30, false, secure);
        let async_graphql::BatchResponse::Single(response) = response.0 else {
            panic!("a single response");
        };
        response
            .http_headers
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().expect("ascii").to_string())
            .collect()
    }

    /// A new kept session's intent.
    fn new_session() -> AuthIntent {
        AuthIntent::SetSession {
            session_token: "rim_sess_t".to_string(),
            csrf_token: "rim_csrf_t".to_string(),
            kept: true,
        }
    }

    #[test]
    fn a_desktop_origin_over_https_gets_cross_site_session_cookies() {
        for origin in [
            "http://tauri.localhost",
            "tauri://localhost",
            " TAURI://LOCALHOST ",
        ] {
            let cookies = auth_cookies_for(new_session(), Some(origin), true);
            assert_eq!(cookies.len(), 2, "{origin}");
            assert!(cookies[0].starts_with("rim_session=rim_sess_t"));
            assert!(cookies[1].starts_with("rim_csrf=rim_csrf_t"));
            for cookie in &cookies {
                assert!(cookie.contains("SameSite=None"), "{cookie}");
                assert!(cookie.contains("Secure"), "{cookie}");
                assert!(!cookie.contains("SameSite=Lax"), "{cookie}");
            }
        }
    }

    #[test]
    fn a_desktop_origin_without_secure_keeps_lax_cookies() {
        let cookies = auth_cookies_for(new_session(), Some("http://tauri.localhost"), false);
        assert_eq!(cookies.len(), 2);
        for cookie in &cookies {
            assert!(cookie.contains("SameSite=Lax"), "{cookie}");
            assert!(!cookie.contains("Secure"), "{cookie}");
        }
    }

    #[test]
    fn a_web_origin_or_no_origin_keeps_lax_cookies() {
        for origin in [
            Some("https://relay.example.org"),
            Some("http://localhost:1420"),
            Some("https://tauri.localhost.example.org"),
            None,
        ] {
            let cookies = auth_cookies_for(new_session(), origin, true);
            assert_eq!(cookies.len(), 2, "{origin:?}");
            for cookie in &cookies {
                assert!(cookie.contains("SameSite=Lax"), "{cookie}");
                assert!(cookie.contains("Secure"), "{cookie}");
            }
        }
    }

    #[test]
    fn a_desktop_sign_out_over_https_clears_cross_site_cookies() {
        let cookies = auth_cookies_for(
            AuthIntent::ClearSession,
            Some("http://tauri.localhost"),
            true,
        );
        assert_eq!(cookies.len(), 2);
        assert!(cookies[0].starts_with("rim_session=;"), "{}", cookies[0]);
        assert!(cookies[1].starts_with("rim_csrf=;"), "{}", cookies[1]);
        for cookie in &cookies {
            assert!(cookie.contains("SameSite=None"), "{cookie}");
            assert!(cookie.contains("Secure"), "{cookie}");
            assert!(cookie.contains("Max-Age=0"), "{cookie}");
        }
        let web = auth_cookies_for(
            AuthIntent::ClearSession,
            Some("https://relay.example.org"),
            true,
        );
        for cookie in &web {
            assert!(cookie.contains("SameSite=Lax"), "{cookie}");
        }
    }

    #[test]
    fn only_a_batch_of_reloads_and_sign_outs_needs_no_session_secret() {
        let needs_none = |queries: &[&str]| {
            let batch = async_graphql::BatchRequest::Batch(
                queries
                    .iter()
                    .map(|q| async_graphql::Request::new(*q))
                    .collect(),
            );
            batch_needs_no_secret(&batch_fields(&batch))
        };
        assert!(needs_none(&["mutation { reloadSession { reloadKey } }"]));
        assert!(needs_none(&["mutation { logout }"]));
        assert!(needs_none(&[
            "mutation { reloadSession { reloadKey } logout }",
            "mutation { logout }"
        ]));
        assert!(!needs_none(&[]));
        assert!(!needs_none(&[
            "mutation { reloadSession { reloadKey } setReloadKey }"
        ]));
        assert!(!needs_none(&["mutation { logout }", "query { me { id } }"]));
        assert!(!needs_none(&["query { logout }"]));
        assert!(!needs_none(&["mutation {"]));
        assert!(!needs_none(&["mutation { ...F }"]));
    }

    #[actix_web::test]
    async fn extract_auth_presents_the_secret_header_and_the_download_variant_none() {
        let req = TestRequest::default()
            .cookie(actix_web::cookie::Cookie::new(
                COOKIE_SESSION,
                "session-token",
            ))
            .insert_header((SESSION_SECRET_HEADER, "the-secret"))
            .to_http_request();
        let mut auth = MockRequestAuthenticator::new();
        auth.expect_authenticate_session_token()
            .withf(|token, secret| {
                *token == Some("session-token")
                    && *secret == SessionSecret::Presented(Some("the-secret".to_string()))
            })
            .times(1)
            .return_once(|_, _| Box::pin(async { Ok(RequestAuthResultDto::default()) }));
        auth.expect_authenticate_session_token()
            .withf(|token, secret| {
                *token == Some("session-token") && *secret == SessionSecret::NotRequired
            })
            .times(1)
            .return_once(|_, _| Box::pin(async { Ok(RequestAuthResultDto::default()) }));
        extract_auth(&req, &auth).await.unwrap();
        extract_auth_without_secret(&req, &auth).await.unwrap();
    }
}
