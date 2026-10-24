//! The client side of the account password for the e2e suites: OPAQUE registration and login
//! through the GraphQL roots, against a schema or a running server, with the cheap
//! key-stretching setting.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rim_auth::opaque::client::{Login, Registration};
use serde_json::{Value, json};

use rim_api::AppSchema;

use super::graphql::exec_ok;
use super::identity::signed_out;

/// The password every account the suites sign up with gets.
pub const TEST_PASSWORD: &[u8] = b"correct horse battery staple";

pub const REGISTRATION_START: &str = r#"
mutation Start($principalId: String!, $request: String!) {
  passwordRegistrationStart(principalId: $principalId, request: $request)
}"#;

pub const LOGIN_START: &str = r#"
mutation Start($principalId: String!, $request: String!) {
  passwordLoginStart(principalId: $principalId, request: $request) { loginId response }
}"#;

pub const SET_PASSWORD_START: &str = r#"
mutation Start($request: String!) { setPasswordStart(registrationRequest: $request) }"#;

pub const SET_PASSWORD: &str = r#"
mutation Set($upload: String!) { setPassword(registrationUpload: $upload) { serverShare } }"#;

/// `loginWithKey` with the password fields, answering everything the payload carries.
pub const LOGIN_WITH_PASSWORD: &str = r#"
mutation Login($proof: SignedStatementInput!, $loginId: ID, $credentialFinalization: String) {
  loginWithKey(proof: $proof, loginId: $loginId, credentialFinalization: $credentialFinalization) {
    userId csrfToken serverShare rewraps mustSetPassword
  }
}"#;

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(text: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(text).expect("base64url")
}

/// A password login the client finished: the variables `loginWithKey` takes, and the session
/// key that opens what it returns.
pub struct PasswordLogin {
    pub login_id: String,
    pub finalization: String,
    pub session_key: Vec<u8>,
}

impl PasswordLogin {
    /// The `loginWithKey` variables for `proof` with this login.
    pub fn variables(&self, proof: Value) -> Value {
        json!({
            "proof": proof,
            "loginId": self.login_id,
            "credentialFinalization": self.finalization,
        })
    }
}

/// The `passwordRegistration` of `password` for `principal_id`, through `schema`.
pub async fn registration(schema: &AppSchema, principal_id: &str, password: &[u8]) -> String {
    let (flow, request) = Registration::start(password);
    let data = exec_ok(
        schema,
        signed_out(
            REGISTRATION_START,
            json!({ "principalId": principal_id, "request": b64(&request) }),
        ),
    )
    .await;
    let response = unb64(
        data["passwordRegistrationStart"]
            .as_str()
            .expect("response"),
    );
    b64(&flow.finish(&response).expect("registration").0)
}

/// Start a password login for `principal_id` through `schema` and finish it on the client;
/// `Ok(None)` when the password is wrong (the client cannot finish), and the server's answer
/// when it refused the start.
pub async fn login(
    schema: &AppSchema,
    principal_id: &str,
    password: &[u8],
) -> Result<Option<PasswordLogin>, async_graphql::Response> {
    let (flow, request) = Login::start(password);
    let response = schema
        .execute(signed_out(
            LOGIN_START,
            json!({ "principalId": principal_id, "request": b64(&request) }),
        ))
        .await;
    if !response.errors.is_empty() {
        return Err(response);
    }
    let data = response.data.into_json().expect("json");
    let started = &data["passwordLoginStart"];
    let Some(done) = flow.finish(&unb64(started["response"].as_str().expect("response"))) else {
        return Ok(None);
    };
    Ok(Some(PasswordLogin {
        login_id: started["loginId"].as_str().expect("loginId").to_string(),
        finalization: b64(&done.finalization),
        session_key: done.session_key,
    }))
}

/// POST `query` with `variables` to `origin`'s `/api` and return the JSON answer.
async fn post(http: &reqwest::Client, origin: &str, query: &str, variables: Value) -> Value {
    http.post(format!("{origin}/api"))
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .await
        .expect("post")
        .json()
        .await
        .expect("json")
}

/// [`registration`] against the server at `origin`.
pub async fn registration_over_http(
    http: &reqwest::Client,
    origin: &str,
    principal_id: &str,
    password: &[u8],
) -> String {
    let (flow, request) = Registration::start(password);
    let body = post(
        http,
        origin,
        REGISTRATION_START,
        json!({ "principalId": principal_id, "request": b64(&request) }),
    )
    .await;
    let response = body["data"]["passwordRegistrationStart"]
        .as_str()
        .unwrap_or_else(|| panic!("passwordRegistrationStart: {body}"));
    b64(&flow.finish(&unb64(response)).expect("registration").0)
}

/// [`login`] against the server at `origin`.
pub async fn login_over_http(
    http: &reqwest::Client,
    origin: &str,
    principal_id: &str,
    password: &[u8],
) -> Option<PasswordLogin> {
    let (flow, request) = Login::start(password);
    let body = post(
        http,
        origin,
        LOGIN_START,
        json!({ "principalId": principal_id, "request": b64(&request) }),
    )
    .await;
    let started = &body["data"]["passwordLoginStart"];
    let done = flow.finish(&unb64(
        started["response"]
            .as_str()
            .unwrap_or_else(|| panic!("passwordLoginStart: {body}")),
    ))?;
    Some(PasswordLogin {
        login_id: started["loginId"].as_str().expect("loginId").to_string(),
        finalization: b64(&done.finalization),
        session_key: done.session_key,
    })
}
