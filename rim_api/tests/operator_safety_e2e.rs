//! Through-the-stack e2e tests for the operator's safety settings: `RIM_MINIMUM_AGE` makes
//! every signup confirm the age and records it on the account; `RIM_TERMS_FILE` serves the
//! terms as a page that is the terms URL; `RIM_ABUSE_CONTACT` reaches `serverInfo` and the
//! signed descriptor.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use reqwest::StatusCode;
use sea_orm::{ConnectionTrait, Statement};
use serde_json::{Value, json};

use rim_identity::StatementVersion;
use rim_identity::testing::TestIdentity;

use support::TestCtx;
use support::graphql::{anonymous_request, exec_err, exec_ok};
use support::identity::{SIGNUP_WITH_KEY, signed_out, signup_input};
use support::nodes::{S2S_TABLES, start};

const SIGNUP_WITH_INVITE: &str = r#"
mutation Signup($input: SignupWithInviteInput!) { signupWithInvite(input: $input) { userId } }"#;

fn password_signup(username: &str, extra: Value) -> async_graphql::Request {
    let mut input = json!({
        "username": username,
        "email": format!("{username}@test.example.com"),
        "password": "ValidPass123!",
    });
    if let (Some(input), Some(extra)) = (input.as_object_mut(), extra.as_object()) {
        input.extend(extra.clone());
    }
    anonymous_request(SIGNUP_WITH_INVITE, json!({ "input": input }))
}

/// The minimum age recorded on the account `user_id`.
async fn recorded_age(ctx: &TestCtx, user_id: &Value) -> Option<i16> {
    let row = ctx
        .conn
        .query_one_raw(Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            r#"SELECT minimum_age_confirmed FROM "user" WHERE id = $1::uuid"#,
            [user_id.as_str().expect("userId").into()],
        ))
        .await
        .expect("query")
        .expect("the account");
    row.try_get("", "minimum_age_confirmed").expect("column")
}

/// With a minimum age of 16, a password signup and a key signup are refused without the
/// confirmation and with another age, and succeed with 16, which the account records;
/// `serverInfo.minimumAge` names it.
#[tokio::test]
async fn every_signup_confirms_the_minimum_age() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema_with_minimum_age(16);

    let info = exec_ok(
        &schema,
        anonymous_request("{ serverInfo { minimumAge } }", json!({})),
    )
    .await;
    assert_eq!(info["serverInfo"]["minimumAge"], json!(16));

    for extra in [json!({}), json!({ "confirmedMinimumAge": 13 })] {
        let (code, message) = exec_err(&schema, password_signup("age-pw", extra)).await;
        assert_eq!(code, "VALIDATION");
        assert!(message.contains("minimum age"), "{message}");
        assert!(!message.contains("terms"), "{message}");
    }
    let data = exec_ok(
        &schema,
        password_signup("age-pw", json!({ "confirmedMinimumAge": 16 })),
    )
    .await;
    assert_eq!(
        recorded_age(&ctx, &data["signupWithInvite"]["userId"]).await,
        Some(16)
    );

    let id = TestIdentity::with_version(62, StatementVersion::V2);
    let input = signup_input(&schema, "age-key", &id, 72, json!({})).await;
    let (code, _) = exec_err(
        &schema,
        signed_out(SIGNUP_WITH_KEY, json!({ "input": input })),
    )
    .await;
    assert_eq!(code, "VALIDATION");
    let input = signup_input(
        &schema,
        "age-key",
        &id,
        72,
        json!({ "confirmedMinimumAge": 16 }),
    )
    .await;
    let data = exec_ok(
        &schema,
        signed_out(SIGNUP_WITH_KEY, json!({ "input": input })),
    )
    .await;
    assert_eq!(
        recorded_age(&ctx, &data["signupWithKey"]["userId"]).await,
        Some(16)
    );
}

/// Without a minimum age a signup records none, whatever it sends.
#[tokio::test]
async fn without_a_minimum_age_nothing_is_recorded() {
    let ctx = TestCtx::new().await;
    let schema = ctx
        .build_schema(rim_services::auth::dto::RegistrationMode::Open)
        .await;
    let data = exec_ok(
        &schema,
        password_signup("no-age", json!({ "confirmedMinimumAge": 16 })),
    )
    .await;
    assert_eq!(
        recorded_age(&ctx, &data["signupWithInvite"]["userId"]).await,
        None
    );
}

/// A server with a terms file serves it as a page at its terms URL, and publishes its abuse
/// contact in `serverInfo` and in the signed descriptor of its discovery document.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_terms_page_and_abuse_contact_reach_clients_and_peers() {
    let ctx = TestCtx::new().await;
    ctx.conn
        .execute_unprepared(S2S_TABLES)
        .await
        .expect("truncate");
    let dir = tempfile::tempdir().expect("dir");
    let terms = dir.path().join("terms.md");
    std::fs::write(
        &terms,
        "<!-- for operators -->\n# Example terms\n\nBe **kind**.\n\n<script>alert(1)</script>\n",
    )
    .expect("write terms");
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let node = start(&url, |config| {
        config.terms_file = Some(terms.display().to_string());
        config.abuse_contact = Some("abuse@rim.example.org".to_string());
        config.minimum_age = Some(16);
    })
    .await;
    let http = reqwest::Client::new();

    let info: Value = http
        .post(format!("{}/api", node.origin))
        .json(&json!({ "query": "{ serverInfo { termsUrl abuseContact minimumAge } }" }))
        .send()
        .await
        .expect("serverInfo")
        .json()
        .await
        .expect("serverInfo json");
    let info = &info["data"]["serverInfo"];
    assert_eq!(info["abuseContact"], json!("abuse@rim.example.org"));
    assert_eq!(info["minimumAge"], json!(16));
    let terms_url = info["termsUrl"].as_str().expect("termsUrl");
    assert!(
        terms_url.starts_with(&format!("{}/api/terms?v=", node.origin)),
        "{terms_url}"
    );

    let page = http.get(terms_url).send().await.expect("terms page");
    assert_eq!(page.status(), StatusCode::OK);
    let header = |name: &str| {
        page.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(header("content-type"), "text/html; charset=utf-8");
    assert!(header("content-security-policy").starts_with("default-src 'none'"));
    let html = page.text().await.expect("terms html");
    assert!(html.contains("<title>Example terms</title>"), "{html}");
    assert!(html.contains("<p>Be <strong>kind</strong>.</p>"), "{html}");
    assert!(html.contains("&lt;script&gt;"), "{html}");
    assert!(!html.contains("for operators"), "{html}");

    let discovery: Value = http
        .get(format!("{}/.well-known/rim-server", node.origin))
        .send()
        .await
        .expect("discovery")
        .json()
        .await
        .expect("discovery json");
    let payload = discovery["descriptor"]["payload"]
        .as_str()
        .expect("descriptor payload");
    assert!(
        payload.ends_with(r#","abuseContact":"abuse@rim.example.org"}"#),
        "{payload}"
    );

    node.stop().await;
}

/// A server without a terms file answers 404 at the terms path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_terms_file_there_is_no_terms_page() {
    let ctx = TestCtx::new().await;
    ctx.conn
        .execute_unprepared(S2S_TABLES)
        .await
        .expect("truncate");
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let node = start(&url, |_| {}).await;
    let res = reqwest::Client::new()
        .get(format!("{}/api/terms", node.origin))
        .send()
        .await
        .expect("terms");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    node.stop().await;
}
