//! The public relay on two RIM servers: proof-of-work signups refused, accepted and replaced by
//! an invite; a reported DM request listed and resolved by the operator; a principal blocked on
//! its relay signed out and refused signup, login and delivery; and a peer blocked both ways.
//! Separate from `relay_e2e.rs` so each binary's servers stay within Postgres's connections.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rim_core::user::model::UserId;
use rim_server::config::RIMConfig;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use support::nodes::{Node, databases_with, start};
use support::relay::{
    CREATE, RESET, drained, key_signup, keys, login_proof, open_registration, relays, sign_up,
    statement,
};

const LOGIN: &str = r#"
mutation Login($proof: SignedStatementInput!) { loginWithKey(proof: $proof) { userId } }"#;

/// The first error code of a GraphQL answer.
fn error_code(body: &Value) -> &str {
    body["errors"][0]["extensions"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error: {body}"))
}

/// The configuration the operator's commands run with on a node's database.
fn operator_config(database_url: &str) -> RIMConfig {
    RIMConfig {
        database_url: database_url.to_string(),
        ..Default::default()
    }
}

async fn operator(database_url: &str, group: &str, args: &[&str]) -> String {
    rim_server::abuse_cmd::run_with(
        &operator_config(database_url),
        group,
        args.iter().map(|a| a.to_string()),
    )
    .await
    .unwrap_or_else(|e| panic!("rim_server {group} {args:?}: {e}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pow_signups_are_accepted_refused_or_take_an_invite() {
    const BITS: u32 = 8;
    let (first, _) = databases_with(RESET).await;
    let node = start(&first, |c| c.registration_pow_bits = Some(BITS)).await;
    let http = reqwest::Client::new();

    // The descriptor and `registrationMode` say how to sign up.
    let discovery: Value = http
        .get(format!("{}/.well-known/rim-server", node.origin))
        .send()
        .await
        .expect("discovery")
        .json()
        .await
        .expect("json");
    let descriptor: Value = serde_json::from_str(
        discovery["descriptor"]["payload"]
            .as_str()
            .expect("payload"),
    )
    .expect("descriptor");
    assert_eq!(
        descriptor["registration"],
        json!({ "mode": "pow", "powBits": BITS })
    );
    let mode: Value = http
        .post(format!("{}/api", node.origin))
        .json(&json!({ "query": "query { registrationMode }" }))
        .send()
        .await
        .expect("mode")
        .json()
        .await
        .expect("json");
    assert_eq!(mode["data"]["registrationMode"], "POW");

    let ada = keys(11);
    let inception = ada.identity.inception().payload;
    let nonce = rim_s2s::pow::solve_pow(&node.id(), &inception, BITS);
    // A weak nonce, one for another server, none at all, and both a token and a nonce: refused.
    let weak = (0u64..)
        .map(u64::to_be_bytes)
        .find(|n| {
            rim_s2s::pow::check_pow(&node.id(), &inception, &URL_SAFE_NO_PAD.encode(n), BITS)
                .is_err()
        })
        .expect("a weak nonce");
    let elsewhere = rim_s2s::pow::solve_pow(
        "rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c",
        &inception,
        BITS,
    );
    for extra in [
        json!({ "proofOfWork": URL_SAFE_NO_PAD.encode(weak) }),
        json!({}),
        json!({ "proofOfWork": URL_SAFE_NO_PAD.encode(nonce), "instanceInviteToken": "rim_inv" }),
    ] {
        let (body, _) = key_signup(&http, &node, "ada", &ada, extra.clone()).await;
        assert_eq!(error_code(&body), "VALIDATION", "{extra}: {body}");
    }
    if rim_s2s::pow::check_pow(
        &node.id(),
        &inception,
        &URL_SAFE_NO_PAD.encode(elsewhere),
        BITS,
    )
    .is_err()
    {
        let (body, _) = key_signup(
            &http,
            &node,
            "ada",
            &ada,
            json!({ "proofOfWork": URL_SAFE_NO_PAD.encode(elsewhere) }),
        )
        .await;
        assert_eq!(error_code(&body), "VALIDATION", "{body}");
    }
    let (body, _) = key_signup(
        &http,
        &node,
        "ada",
        &ada,
        json!({ "proofOfWork": URL_SAFE_NO_PAD.encode(nonce) }),
    )
    .await;
    assert!(body.get("errors").is_none(), "{body}");
    let ada_id: UserId = body["data"]["signupWithKey"]["userId"]
        .as_str()
        .expect("user id")
        .parse::<Uuid>()
        .expect("uuid")
        .into();

    // An instance invite stands in for the work.
    use rim_core::auth::repository::InstanceInviteRepository;
    rim_infra_db::sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository::new(
        node.db.clone().into(),
    )
    .create(rim_core::auth::model::NewInstanceInvite {
        created_by_user_id: ada_id,
        token_hash: rim_auth::session_token::hash_token("rim_inv_grace"),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        max_uses: 1,
    })
    .await
    .expect("instance invite");
    let (body, _) = key_signup(
        &http,
        &node,
        "grace",
        &keys(12),
        json!({ "instanceInviteToken": "rim_inv_grace" }),
    )
    .await;
    assert!(body.get("errors").is_none(), "{body}");
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reported_request_is_listed_and_a_blocked_principal_is_signed_out_and_refused() {
    let (first, second) = databases_with(RESET).await;
    let a = start(&first, open_registration).await;
    let b = start(&second, open_registration).await;
    let alice = sign_up(&a, "alice", 1).await;
    let bob = sign_up(&b, "bob", 2).await;

    // Bob reports Alice's request; the operator of his relay lists and resolves it.
    alice.start_dm(&bob, &b, true).await;
    let request = bob.pending_request(&alice).await;
    let reported = bob
        .ok(
            "mutation($id: ID!, $r: String!) { reportDmRequest(id: $id, reason: $r) }",
            json!({ "id": request["id"], "r": "unsolicited ads" }),
        )
        .await;
    assert_eq!(reported["reportDmRequest"], json!(true));
    let listed = operator(&second, "abuse-reports", &["list", "--open"]).await;
    let request_id = request["id"].as_str().expect("id");
    for needle in [
        alice.principal(),
        a.id(),
        request_id.to_string(),
        "unsolicited ads".to_string(),
        "open".to_string(),
    ] {
        assert!(listed.contains(&needle), "{needle} in {listed}");
    }
    let report_id = listed.split_whitespace().next().expect("report id");
    let resolved = operator(&second, "abuse-reports", &["resolve", report_id]).await;
    assert!(resolved.contains("resolved"), "{resolved}");
    assert_eq!(
        operator(&second, "abuse-reports", &["list", "--open"]).await,
        ""
    );

    // The operator blocks Bob: his session ends and he cannot sign in again.
    let blocked = operator(
        &second,
        "principal",
        &["block", &bob.principal(), "--reason", "spam wave"],
    )
    .await;
    assert!(blocked.contains("ended 1 sessions"), "{blocked}");
    let body = bob
        .post("query { contacts { principalId } }", json!({}))
        .await;
    assert_eq!(error_code(&body), "UNAUTHENTICATED", "{body}");
    let http = reqwest::Client::new();
    let proof = login_proof(&http, &b, &keys(2)).await;
    let body: Value = http
        .post(format!("{}/api", b.origin))
        .json(&json!({ "query": LOGIN, "variables": { "proof": statement(&proof) } }))
        .send()
        .await
        .expect("login")
        .json()
        .await
        .expect("json");
    assert_eq!(error_code(&body), "FORBIDDEN", "{body}");
    assert!(
        operator(&second, "principal", &["list"])
            .await
            .contains(&bob.principal())
    );

    // A blocked principal cannot sign up.
    operator(
        &second,
        "principal",
        &[
            "block",
            &keys(3).state.principal_id,
            "--reason",
            "known spammer",
        ],
    )
    .await;
    let (body, _) = key_signup(&http, &b, "carol", &keys(3), json!({})).await;
    assert_eq!(error_code(&body), "FORBIDDEN", "{body}");

    // Nothing more is delivered to Bob: a new conversation from Alice files no request.
    let direct = alice.direct_space().await;
    let group = alice
        .ok(
            CREATE,
            json!({ "input": {
                "serverId": direct,
                "memberIds": [],
                "remoteMembers": [{ "principalId": bob.principal(), "relayUrls": [b.origin] }],
                "title": "after the block",
                "encrypted": true,
            } }),
        )
        .await["createConversation"]["id"]
        .as_str()
        .expect("group id")
        .to_string();
    drained(&a).await;
    let filed =
        b.db.query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM relay_dm_request WHERE conversation_id = $1",
            [group.parse::<Uuid>().expect("uuid").into()],
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get::<i64>("", "n")
        .expect("n");
    assert_eq!(filed, 0, "a blocked recipient gets nothing");

    // Unblocked, Bob signs in again.
    let unblocked = operator(&second, "principal", &["unblock", &bob.principal()]).await;
    assert!(unblocked.contains("unblocked"), "{unblocked}");
    let proof = login_proof(&http, &b, &keys(2)).await;
    let body: Value = http
        .post(format!("{}/api", b.origin))
        .json(&json!({ "query": LOGIN, "variables": { "proof": statement(&proof) } }))
        .send()
        .await
        .expect("login")
        .json()
        .await
        .expect("json");
    assert!(body.get("errors").is_none(), "{body}");
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blocked_peer_gets_403_both_ways_until_unblocked() {
    use rim_services::s2s::dto::{PeerBlockChange, S2sClientError, S2sMethod, S2sRequest};

    let (a, b) = relays().await;
    let ping = |to: &Node| S2sRequest {
        peer_server_id: to.id(),
        method: S2sMethod::Get,
        path: "/ping".into(),
        body: vec![],
        content_type: None,
        headers: vec![],
    };
    // B pins A and pings it, so A pins B.
    b.s2s
        .state
        .peers
        .pin(&a.origin, Some(&a.id()))
        .await
        .expect("pin");
    let answer = b
        .s2s
        .client
        .send(ping(&a))
        .await
        .expect("a verified answer");
    assert_eq!(answer.status, 200);

    let peers = &a.s2s.state.peers;
    assert_eq!(
        peers.set_blocked(&b.id(), true).await.expect("block"),
        PeerBlockChange::Changed
    );
    assert_eq!(
        peers.set_blocked(&b.id(), true).await.expect("again"),
        PeerBlockChange::Unchanged
    );
    let refused = b.s2s.client.send(ping(&a)).await.expect("a signed refusal");
    assert_eq!(refused.status, 403);
    let body: Value = serde_json::from_slice(&refused.body).expect("json");
    assert_eq!(body["error"], "peer_not_allowed");
    // A no longer talks to B either.
    assert!(matches!(
        a.s2s.client.send(ping(&b)).await,
        Err(S2sClientError::PeerBlocked)
    ));

    assert_eq!(
        peers.set_blocked(&b.id(), false).await.expect("unblock"),
        PeerBlockChange::Changed
    );
    let answer = b
        .s2s
        .client
        .send(ping(&a))
        .await
        .expect("a verified answer");
    assert_eq!(answer.status, 200);
    assert_eq!(
        peers
            .set_blocked("rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw", true)
            .await
            .expect("unknown"),
        PeerBlockChange::NotPinned
    );
    a.stop().await;
    b.stop().await;
}
