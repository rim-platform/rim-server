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

pub const LOGIN_FINISH: &str = r#"
mutation Finish($loginId: ID!, $credentialFinalization: String!) {
  passwordLoginFinish(loginId: $loginId, credentialFinalization: $credentialFinalization) {
    ticket serverShare rewraps
  }
}"#;

/// `loginWithKey` with an optional password ticket, kept on a client that holds its own keep
/// key unless the variables say otherwise.
pub const LOGIN_WITH_PASSWORD: &str = r#"
mutation Login($proof: SignedStatementInput!, $passwordTicket: String,
               $keepSignedIn: Boolean! = true, $keepKeyHeld: Boolean! = true) {
  loginWithKey(proof: $proof, passwordTicket: $passwordTicket,
               keepSignedIn: $keepSignedIn, keepKeyHeld: $keepKeyHeld) {
    userId csrfToken mustSetPassword sessionSecret keepKey
  }
}"#;

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(text: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(text).expect("base64url")
}

/// A password login that finished: the ticket `loginWithKey` takes, what
/// `passwordLoginFinish` answered, and the client's session key, which opens the share.
pub struct PasswordLogin {
    pub ticket: String,
    /// The server share, sealed under the login's transport key.
    pub server_share: String,
    pub rewraps: Value,
    pub session_key: Vec<u8>,
}

impl PasswordLogin {
    /// The `loginWithKey` variables for `proof` with this login's ticket.
    pub fn variables(&self, proof: Value) -> Value {
        json!({ "proof": proof, "passwordTicket": self.ticket })
    }
}

/// The started login and the `passwordLoginFinish` answer as a [`PasswordLogin`].
fn finished(finish: &Value, session_key: Vec<u8>) -> PasswordLogin {
    PasswordLogin {
        ticket: finish["ticket"].as_str().expect("ticket").to_string(),
        server_share: finish["serverShare"]
            .as_str()
            .expect("serverShare")
            .to_string(),
        rewraps: finish["rewraps"].clone(),
        session_key,
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

/// Run a password login for `principal_id` through `schema`: start it, finish it on the
/// client, and finish it on the server. `Ok(None)` when the password is wrong (the client
/// cannot finish), and the server's answer when it refused a step.
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
    let response = schema
        .execute(signed_out(
            LOGIN_FINISH,
            json!({
                "loginId": started["loginId"],
                "credentialFinalization": b64(&done.finalization),
            }),
        ))
        .await;
    if !response.errors.is_empty() {
        return Err(response);
    }
    let data = response.data.into_json().expect("json");
    Ok(Some(finished(
        &data["passwordLoginFinish"],
        done.session_key,
    )))
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

/// [`login`] against the server at `origin`; `None` when the password is wrong.
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
    let body = post(
        http,
        origin,
        LOGIN_FINISH,
        json!({
            "loginId": started["loginId"],
            "credentialFinalization": b64(&done.finalization),
        }),
    )
    .await;
    let finish = &body["data"]["passwordLoginFinish"];
    assert!(finish.is_object(), "passwordLoginFinish: {body}");
    Some(finished(finish, done.session_key))
}
