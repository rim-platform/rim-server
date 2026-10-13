//! The instance moderation queue over a real database: abuse reports, principal blocks and IP
//! blocks as an instance operator reads and changes them through GraphQL, and the refusal
//! everyone else gets.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests --test instance_moderation_e2e -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::sync::Arc;

use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::user::model::User;
use rim_services::auth::dto::RegistrationMode;
use rim_services::operator::OperatorCaller;
use rim_services::provider::AppServiceProvider;
use rim_services::relay::dto::FileAbuseReport;

use support::graphql::{Caller, anonymous_request, assert_each_refused, exec_err_code, exec_ok};
use support::{TestCtx, unknown_id};

/// The principal the member's account holds.
const MEMBER_PRINCIPAL: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";
/// A principal no account here holds.
const STRANGER: &str = "rimp1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";

const ME: &str = "query { me { username instanceOperator } }";

const REPORTS: &str = r#"query {
  instanceReports {
    id reporterUserId reporterUsername subjectPrincipalId subjectUsername subjectServerId
    requestId reason createdAt resolvedAt resolved
  }
}"#;

const ALL_REPORTS: &str = "query { instanceReports(openOnly: false) { id resolved } }";

const RESOLVE: &str = r#"mutation($id: ID!) {
  instanceResolveReport(id: $id) { id resolved resolvedAt subjectUsername }
}"#;

const PRINCIPAL_BLOCKS: &str =
    "query { instancePrincipalBlocks { principalId username reason createdAt } }";

const BLOCK_PRINCIPAL: &str = r#"mutation($p: String!, $r: String!) {
  instanceBlockPrincipal(principalId: $p, reason: $r) {
    principalId newlyBlocked reason endedSessions ignoredRequests revokedInvites emailBlocked
    account { username principalId } invitees { username }
  }
}"#;

const UNBLOCK_PRINCIPAL: &str = r#"mutation($p: String!) {
  instanceUnblockPrincipal(principalId: $p) { lifted emailUnblocked }
}"#;

const IP_BLOCKS: &str = "query { instanceIpBlocks { cidr reason expiresAt createdAt expired } }";

const BLOCK_IP: &str = r#"mutation($cidr: String!, $r: String!, $days: Int) {
  instanceBlockIp(cidr: $cidr, reason: $r, expiresInDays: $days) {
    newlyBlocked block { cidr reason expiresAt expired }
  }
}"#;

const UNBLOCK_IP: &str = "mutation($cidr: String!) { instanceUnblockIp(cidr: $cidr) }";

/// A schema over a fresh database with an instance operator and a plain member whose account
/// holds [`MEMBER_PRINCIPAL`].
struct Queue {
    schema: AppSchema,
    provider: Arc<dyn AppServiceProvider>,
    operator: Caller,
    member: Caller,
}

impl Queue {
    async fn new() -> Self {
        let ctx = TestCtx::new().await;
        let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
        let provider = ctx.build_provider();
        let operator = ctx.create_user("opal").await;
        let member = ctx.create_user("milo").await;
        hold_principal(&ctx, &member, MEMBER_PRINCIPAL).await;
        provider
            .instance_operator_service()
            .grant("opal".to_string())
            .await
            .expect("grant");
        Self {
            schema,
            provider,
            operator: ctx.caller(&operator).await,
            member: ctx.caller(&member).await,
        }
    }

    /// File a report from the abuse address about `principal_id`; its id.
    async fn report(&self, principal_id: &str, reason: &str) -> String {
        self.provider
            .relay_abuse_service()
            .file_report(
                OperatorCaller::CommandLine,
                FileAbuseReport {
                    subject_principal_id: Some(principal_id.to_string()),
                    subject_server_id: None,
                    reason: reason.to_string(),
                },
            )
            .await
            .expect("file report")
            .id
            .to_string()
    }

    async fn as_operator(&self, query: &str, variables: Value) -> Value {
        exec_ok(&self.schema, self.operator.request(query, variables)).await
    }
}

/// Make `user`'s account hold `principal_id`, as a key enrollment would.
async fn hold_principal(ctx: &TestCtx, user: &User, principal_id: &str) {
    ctx.conn
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"UPDATE "user" SET principal_id = $1, identity_key = 'ed25519:e2e' WHERE id = $2"#,
            vec![principal_id.into(), uuid::Uuid::from(user.id).into()],
        ))
        .await
        .expect("hold principal");
}

#[tokio::test]
async fn me_says_whether_the_viewer_is_an_instance_operator() {
    let q = Queue::new().await;
    let me = q.as_operator(ME, json!({})).await;
    assert_eq!(
        me["me"],
        json!({ "username": "opal", "instanceOperator": true })
    );
    let me = exec_ok(&q.schema, q.member.request(ME, json!({}))).await;
    assert_eq!(
        me["me"],
        json!({ "username": "milo", "instanceOperator": false })
    );
}

#[tokio::test]
async fn an_operator_lists_reports_naming_local_accounts_and_resolves_one() {
    let q = Queue::new().await;
    let about_member = q.report(MEMBER_PRINCIPAL, "spam from milo").await;
    q.report(STRANGER, "spam from afar").await;

    let open = q.as_operator(REPORTS, json!({})).await;
    let open = open["instanceReports"].as_array().expect("reports").clone();
    assert_eq!(open.len(), 2, "{open:?}");
    assert_eq!(open[0]["id"], json!(about_member));
    assert_eq!(open[0]["subjectPrincipalId"], json!(MEMBER_PRINCIPAL));
    assert_eq!(open[0]["subjectUsername"], json!("milo"));
    assert_eq!(open[0]["reporterUserId"], Value::Null);
    assert_eq!(open[0]["reason"], json!("spam from milo"));
    assert_eq!(open[0]["resolved"], json!(false));
    assert_eq!(open[1]["subjectUsername"], Value::Null);

    let resolved = q.as_operator(RESOLVE, json!({ "id": about_member })).await;
    assert_eq!(resolved["instanceResolveReport"]["resolved"], json!(true));
    assert_eq!(
        resolved["instanceResolveReport"]["subjectUsername"],
        json!("milo")
    );
    let open = q.as_operator(REPORTS, json!({})).await;
    assert_eq!(open["instanceReports"].as_array().map(Vec::len), Some(1));
    let all = q.as_operator(ALL_REPORTS, json!({})).await;
    assert_eq!(all["instanceReports"].as_array().map(Vec::len), Some(2));

    let missing = exec_err_code(
        &q.schema,
        q.operator.request(RESOLVE, json!({ "id": unknown_id() })),
    )
    .await;
    assert_eq!(missing, "NOT_FOUND");
}

#[tokio::test]
async fn an_operator_blocks_a_local_principal_lists_the_block_and_lifts_it() {
    let q = Queue::new().await;
    let blocked = q
        .as_operator(
            BLOCK_PRINCIPAL,
            json!({ "p": MEMBER_PRINCIPAL, "r": "spam wave" }),
        )
        .await;
    let outcome = &blocked["instanceBlockPrincipal"][0];
    assert_eq!(outcome["principalId"], json!(MEMBER_PRINCIPAL));
    assert_eq!(outcome["newlyBlocked"], json!(true));
    assert_eq!(outcome["account"]["username"], json!("milo"));
    assert_eq!(outcome["emailBlocked"], json!(true));
    assert_eq!(outcome["endedSessions"], json!(1));
    assert_eq!(outcome["invitees"], json!([]));

    let blocks = q.as_operator(PRINCIPAL_BLOCKS, json!({})).await;
    let blocks = blocks["instancePrincipalBlocks"]
        .as_array()
        .expect("blocks")
        .clone();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["principalId"], json!(MEMBER_PRINCIPAL));
    assert_eq!(blocks[0]["username"], json!("milo"));
    assert_eq!(blocks[0]["reason"], json!("spam wave"));

    let lifted = q
        .as_operator(UNBLOCK_PRINCIPAL, json!({ "p": MEMBER_PRINCIPAL }))
        .await;
    assert_eq!(
        lifted["instanceUnblockPrincipal"],
        json!({ "lifted": true, "emailUnblocked": true })
    );
    let blocks = q.as_operator(PRINCIPAL_BLOCKS, json!({})).await;
    assert_eq!(blocks["instancePrincipalBlocks"], json!([]));
}

#[tokio::test]
async fn an_operator_blocks_an_ip_range_lists_it_and_lifts_it() {
    let q = Queue::new().await;
    let blocked = q
        .as_operator(
            BLOCK_IP,
            json!({ "cidr": "203.0.113.77/24", "r": "scraping", "days": 7 }),
        )
        .await;
    let outcome = &blocked["instanceBlockIp"];
    assert_eq!(outcome["newlyBlocked"], json!(true));
    assert_eq!(outcome["block"]["cidr"], json!("203.0.113.0/24"));
    assert_eq!(outcome["block"]["expired"], json!(false));
    assert!(outcome["block"]["expiresAt"].is_string(), "{outcome}");

    let blocks = q.as_operator(IP_BLOCKS, json!({})).await;
    let blocks = blocks["instanceIpBlocks"]
        .as_array()
        .expect("blocks")
        .clone();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["cidr"], json!("203.0.113.0/24"));
    assert_eq!(blocks[0]["reason"], json!("scraping"));

    let lifted = q
        .as_operator(UNBLOCK_IP, json!({ "cidr": "203.0.113.0/24" }))
        .await;
    assert_eq!(lifted["instanceUnblockIp"], json!(true));
    let again = q
        .as_operator(UNBLOCK_IP, json!({ "cidr": "203.0.113.0/24" }))
        .await;
    assert_eq!(again["instanceUnblockIp"], json!(false));
    let blocks = q.as_operator(IP_BLOCKS, json!({})).await;
    assert_eq!(blocks["instanceIpBlocks"], json!([]));
}

#[tokio::test]
async fn every_instance_root_is_forbidden_to_a_member_and_unauthenticated_without_a_session() {
    let q = Queue::new().await;
    let report = q.report(STRANGER, "spam").await;
    let calls = [
        (REPORTS, json!({})),
        (ALL_REPORTS, json!({})),
        (PRINCIPAL_BLOCKS, json!({})),
        (IP_BLOCKS, json!({})),
        (BLOCK_PRINCIPAL, json!({ "p": STRANGER, "r": "spam" })),
        (UNBLOCK_PRINCIPAL, json!({ "p": STRANGER })),
        (RESOLVE, json!({ "id": report })),
        (BLOCK_IP, json!({ "cidr": "198.51.100.1", "r": "spam" })),
        (UNBLOCK_IP, json!({ "cidr": "198.51.100.1" })),
    ];
    assert_each_refused(&q.schema, &calls, "FORBIDDEN", |query, variables| {
        q.member.request(query, variables)
    })
    .await;
    assert_each_refused(&q.schema, &calls, "UNAUTHENTICATED", anonymous_request).await;

    // Nothing the refused calls asked for happened.
    assert_eq!(
        q.as_operator(PRINCIPAL_BLOCKS, json!({})).await["instancePrincipalBlocks"],
        json!([])
    );
    assert_eq!(
        q.as_operator(IP_BLOCKS, json!({})).await["instanceIpBlocks"],
        json!([])
    );
    let open = q.as_operator(REPORTS, json!({})).await;
    assert_eq!(open["instanceReports"][0]["resolved"], json!(false));
}
