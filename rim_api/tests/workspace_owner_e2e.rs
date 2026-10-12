//! The owner's way out of a workspace over a real database: `transferWorkspaceOwnership` and
//! `deleteWorkspace`.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests --test workspace_owner_e2e -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use serde_json::json;

use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{exec_err, exec_err_code, exec_ok};

const TRANSFER: &str = r#"mutation($s: ID!, $u: ID!) {
  transferWorkspaceOwnership(input: { serverId: $s, userId: $u }) { id viewerRole }
}"#;

const DELETE: &str = r#"mutation($s: ID!, $n: String!) {
  deleteWorkspace(input: { serverId: $s, confirmName: $n }) { deleted }
}"#;

const MEMBERS: &str = "query($s: ID!) { serverMembers(serverId: $s) { userId memberRole } }";

const SERVER: &str = "query($s: ID!) { server(serverId: $s) { id } }";

const MY_SERVERS: &str = "query { myServers { id } }";

/// Only the owner hands the workspace over, and only to a member who is not a guest; the new
/// owner holds the one owner role and the old one is a moderator who can no longer hand it on.
#[tokio::test]
async fn the_owner_hands_the_workspace_to_a_member_and_becomes_a_moderator() {
    let ctx = TestCtx::new().await;
    let roles = ctx.roles("transfer").await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let s = roles.server_id.clone();
    let to = |who: &support::graphql::Caller| json!({ "s": s, "u": who.user_id.to_string() });

    assert_eq!(
        exec_err_code(&schema, roles.mona.request(TRANSFER, to(&roles.bob))).await,
        "FORBIDDEN"
    );
    assert_eq!(
        exec_err_code(&schema, roles.alice.request(TRANSFER, to(&roles.gina))).await,
        "VALIDATION"
    );
    assert_eq!(
        exec_err_code(&schema, roles.alice.request(TRANSFER, to(&roles.olive))).await,
        "NOT_FOUND"
    );

    let data = exec_ok(&schema, roles.alice.request(TRANSFER, to(&roles.bob))).await;
    assert_eq!(data["transferWorkspaceOwnership"]["id"], json!(s));
    assert_eq!(
        data["transferWorkspaceOwnership"]["viewerRole"],
        json!("MODERATOR")
    );
    let members = exec_ok(&schema, roles.bob.request(MEMBERS, json!({ "s": s }))).await;
    let role_of = |who: &support::graphql::Caller| {
        members["serverMembers"]
            .as_array()
            .expect("members")
            .iter()
            .find(|m| m["userId"] == json!(who.user_id.to_string()))
            .map(|m| m["memberRole"].clone())
    };
    assert_eq!(role_of(&roles.bob), Some(json!("OWNER")));
    assert_eq!(role_of(&roles.alice), Some(json!("MODERATOR")));
    let owners = members["serverMembers"]
        .as_array()
        .expect("members")
        .iter()
        .filter(|m| m["memberRole"] == json!("OWNER"))
        .count();
    assert_eq!(owners, 1);
    assert_eq!(
        exec_err_code(&schema, roles.alice.request(TRANSFER, to(&roles.carol))).await,
        "FORBIDDEN"
    );
}

/// The owner deletes the workspace by typing its name; a wrong name or another member is
/// refused, and afterwards the members no longer have it.
#[tokio::test]
async fn the_owner_deletes_the_workspace_with_its_name_and_its_members_lose_it() {
    let ctx = TestCtx::new().await;
    let roles = ctx.roles("delete").await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let s = roles.server_id.clone();
    let name = "delete-e2e";

    let (code, message) = exec_err(
        &schema,
        roles
            .alice
            .request(DELETE, json!({ "s": s, "n": "Delete-E2E" })),
    )
    .await;
    assert_eq!(code, "VALIDATION");
    assert!(message.contains("name"), "{message}");
    assert_eq!(
        exec_err_code(
            &schema,
            roles.mona.request(DELETE, json!({ "s": s, "n": name }))
        )
        .await,
        "FORBIDDEN"
    );

    let data = exec_ok(
        &schema,
        roles.alice.request(DELETE, json!({ "s": s, "n": name })),
    )
    .await;
    assert_eq!(data["deleteWorkspace"]["deleted"], json!(true));

    for who in [&roles.alice, &roles.bob] {
        assert_eq!(
            exec_err_code(&schema, who.request(SERVER, json!({ "s": s }))).await,
            "FORBIDDEN"
        );
        let mine = exec_ok(&schema, who.request(MY_SERVERS, json!({}))).await;
        assert_eq!(mine["myServers"], json!([]));
    }
    // The other workspace is untouched.
    let mine = exec_ok(&schema, roles.olive.request(MY_SERVERS, json!({}))).await;
    assert_eq!(mine["myServers"], json!([{ "id": roles.other_server_id }]));
}
