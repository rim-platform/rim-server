/// Helpers for executing GraphQL against an [`AppSchema`] as a given user.
///
/// The HTTP layer (`rim_server`) resolves the session cookie to an [`AuthActorCtx`] and attaches
/// it to the request; these helpers attach the same context directly, so everything from the
/// resolver down runs exactly as it does for a real client.
use async_graphql::{Request, Variables};
use serde_json::Value;

use rim_api::AppSchema;
use rim_api::auth_context::AuthActorCtx;
use rim_core::auth::model::SessionId;
use rim_core::user::model::UserId;

/// An authenticated caller: a user plus one of their live sessions.
#[derive(Clone, Copy, Debug)]
pub struct Caller {
    pub user_id: UserId,
    pub session_id: SessionId,
}

impl Caller {
    /// Build a request carrying this caller's auth context and the given JSON `variables`.
    pub fn request(&self, query: &str, variables: Value) -> Request {
        Request::new(query)
            .variables(Variables::from_json(variables))
            .data(AuthActorCtx::new(Some(self.user_id), Some(self.session_id)))
    }
}

/// Build a request with no authenticated actor (no or invalid session cookie).
pub fn anonymous_request(query: &str, variables: Value) -> Request {
    Request::new(query)
        .variables(Variables::from_json(variables))
        .data(AuthActorCtx::new(None, None))
}

/// Execute `request`, panic on any GraphQL error, and return the `data` object as JSON.
pub async fn exec_ok(schema: &AppSchema, request: Request) -> Value {
    let res = schema.execute(request).await;
    assert!(
        res.errors.is_empty(),
        "unexpected GraphQL errors: {:?}",
        res.errors
    );
    res.data
        .into_json()
        .expect("data must be JSON-serialisable")
}

/// Execute `request`, assert it failed, and return the first error's `extensions.code`.
pub async fn exec_err_code(schema: &AppSchema, request: Request) -> String {
    exec_err(schema, request).await.0
}

/// Execute `request`, assert it failed, and return the first error's `extensions.code` and
/// message.
pub async fn exec_err(schema: &AppSchema, request: Request) -> (String, String) {
    let res = schema.execute(request).await;
    let err = res
        .errors
        .first()
        .unwrap_or_else(|| panic!("expected a GraphQL error, got data: {:?}", res.data));
    match err.extensions.as_ref().and_then(|e| e.get("code")) {
        Some(async_graphql::Value::String(code)) => (code.clone(), err.message.clone()),
        other => panic!("error {err:?} has no string `code` extension (got {other:?})"),
    }
}

/// Execute `request`, assert it failed, and return the first error's message.
pub async fn exec_err_message(schema: &AppSchema, request: Request) -> String {
    let res = schema.execute(request).await;
    res.errors
        .first()
        .unwrap_or_else(|| panic!("expected a GraphQL error, got data: {:?}", res.data))
        .message
        .clone()
}

/// Assert that each `(query, variables)` call, sent through `request`, fails with `code`.
pub async fn assert_each_refused(
    schema: &AppSchema,
    calls: &[(&str, Value)],
    code: &str,
    request: impl Fn(&str, Value) -> Request,
) {
    for (query, variables) in calls {
        let got = exec_err_code(schema, request(query, variables.clone())).await;
        assert_eq!(got, code, "{query}");
    }
}
