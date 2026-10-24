/// Password accounts that sign in through `login`, and requests carrying the session cookie a
/// login hands out.
use std::sync::Arc;

use async_graphql::Request;
use serde_json::json;

use rim_api::AppSchema;
use rim_api::auth_context::AuthActorCtx;
use rim_api::auth_intent::{AuthIntent, AuthIntentCtx};
use rim_auth::password::hash_password;
use rim_core::user::model::{NewUser, User};
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_services::provider::AppServiceProvider;

use super::TestCtx;
use super::graphql::{anonymous_request, exec_ok};

/// Every password account's password.
pub const PASSWORD: &str = "correct horse battery";

pub const LOGIN: &str = r#"
mutation Login($input: LoginInput!) { login(input: $input) { userId csrfToken } }"#;

impl TestCtx {
    /// A user named `username` who signs in with [`PASSWORD`].
    pub async fn create_password_user(&self, username: &str) -> User {
        SeaOrmUserRepository::new(self.db.clone())
            .create_user(NewUser {
                username: username.to_string(),
                password: Some(hash_password(PASSWORD).expect("hash")),
                email: Some(format!("{username}@test.example.com")),
                display_name: None,
                key_identity: None,
            })
            .await
            .expect("create_user")
    }
}

/// Sign in with the password and return the new session token.
pub async fn login(schema: &AppSchema, username: &str) -> String {
    let intent = Arc::new(AuthIntentCtx::default());
    let request = anonymous_request(
        LOGIN,
        json!({ "input": { "username": username, "password": PASSWORD } }),
    )
    .data(Arc::clone(&intent));
    exec_ok(schema, request).await;
    let Some(AuthIntent::SetSession { session_token, .. }) = intent.take() else {
        panic!("login must set a session");
    };
    session_token
}

/// A request carrying `session_token` as the session cookie, resolved the way the HTTP layer
/// resolves it: a revoked or unknown session yields no actor.
pub async fn with_session(
    provider: &Arc<dyn AppServiceProvider>,
    session_token: &str,
    query: &str,
) -> Request {
    let auth = provider
        .request_authenticator()
        .authenticate_session_token(
            Some(session_token),
            rim_services::auth::dto::SessionSecret::NotRequired,
        )
        .await
        .expect("authenticate");
    Request::new(query).data(AuthActorCtx::new(auth.user_id, auth.session_id))
}
