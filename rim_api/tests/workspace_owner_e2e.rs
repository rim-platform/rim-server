//! The owner's way out of a workspace over a real database: `transferWorkspaceOwnership`,
//! `deleteWorkspace`, `leaveWorkspace` and `renameWorkspace`.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests --test workspace_owner_e2e -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde_json::json;

use rim_services::auth::dto::RegistrationMode;
use rim_services::events::WorkspaceEvent;

use support::TestCtx;
use support::graphql::{exec_err, exec_err_code, exec_ok};
use support::roles::create_channel;

const TRANSFER: &str = r#"mutation($s: ID!, $u: ID!) {
  transferWorkspaceOwnership(input: { serverId: $s, userId: $u }) { id viewerRole }
}"#;

const DELETE: &str = r#"mutation($s: ID!, $n: String!) {
  deleteWorkspace(input: { serverId: $s, confirmName: $n }) { deleted }
}"#;

const MEMBERS: &str = "query($s: ID!) { serverMembers(serverId: $s) { userId memberRole } }";

const SERVER: &str = "query($s: ID!) { server(serverId: $s) { id } }";

const MY_SERVERS: &str = "query { myServers { id } }";

const LEAVE: &str = r#"mutation($s: ID!) { leaveWorkspace(serverId: $s) }"#;

const RENAME: &str =
    r#"mutation($s: ID!, $n: String!) { renameWorkspace(serverId: $s, name: $n) { id name } }"#;

const MY_SERVER_NAMES: &str = "query { myServers { id name } }";

const CREATE_SERVER: &str =
    r#"mutation($name: String!) { createServer(input: { name: $name }) { id } }"#;

const SEND: &str = r#"mutation($input: SendMessageInput!) { sendMessage(input: $input) { id } }"#;

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

/// The audit rows of `command_id` in `server`, oldest first, as
/// `(actor, initiator)`.
async fn audit_rows(ctx: &TestCtx, server: &str, command_id: &str) -> Vec<(uuid::Uuid, String)> {
    let server: uuid::Uuid = server.parse().expect("server id");
    entity::command_audit_log::Entity::find()
        .filter(entity::command_audit_log::Column::ServerId.eq(server))
        .filter(entity::command_audit_log::Column::CommandId.eq(command_id))
        .order_by_asc(entity::command_audit_log::Column::Id)
        .all(&ctx.conn)
        .await
        .expect("audit rows")
        .into_iter()
        .map(|row| (row.actor_user_id, row.initiator))
        .collect()
}

/// The `(user, cause)` of every `MemberLeft` recorded.
fn departures(events: &[WorkspaceEvent]) -> Vec<(uuid::Uuid, Option<String>)> {
    events
        .iter()
        .filter_map(|e| match e {
            WorkspaceEvent::MemberLeft { user_id, cause, .. } => {
                Some(((*user_id).into(), cause.clone()))
            }
            _ => None,
        })
        .collect()
}

/// The user ids `serverMembers` lists for `server`, read by `who`.
async fn member_ids(
    schema: &rim_api::AppSchema,
    who: &support::graphql::Caller,
    server: &str,
) -> Vec<String> {
    exec_ok(schema, who.request(MEMBERS, json!({ "s": server }))).await["serverMembers"]
        .as_array()
        .expect("members")
        .iter()
        .map(|m| m["userId"].as_str().expect("user id").to_string())
        .collect()
}

/// A member and a guest leave: they lose the workspace and its conversations, their messages
/// stay, members are told with cause `left`, and each leave is audited. The owner and an
/// outsider cannot leave.
#[tokio::test]
async fn a_member_leaves_and_loses_the_workspace_while_its_messages_stay() {
    let ctx = TestCtx::new().await;
    let roles = ctx.roles("leave").await;
    let (schema, _provider, events) = ctx.build_recording(RegistrationMode::InviteOnly);
    let s = roles.server_id.clone();

    let channel = create_channel(&schema, &roles.bob, &s, "leaving", false).await;
    exec_ok(
        &schema,
        roles.bob.request(
            SEND,
            json!({ "input": { "serverId": s, "conversationId": channel, "text": "still here" } }),
        ),
    )
    .await;

    let (code, message) = exec_err(&schema, roles.alice.request(LEAVE, json!({ "s": s }))).await;
    assert_eq!(code, "VALIDATION");
    assert!(message.contains("transfer ownership"), "{message}");
    assert_eq!(
        exec_err_code(&schema, roles.olive.request(LEAVE, json!({ "s": s }))).await,
        "FORBIDDEN"
    );

    let data = exec_ok(&schema, roles.bob.request(LEAVE, json!({ "s": s }))).await;
    assert_eq!(data, json!({ "leaveWorkspace": true }));
    assert_eq!(
        exec_err_code(&schema, roles.bob.request(SERVER, json!({ "s": s }))).await,
        "FORBIDDEN"
    );
    let mine = exec_ok(&schema, roles.bob.request(MY_SERVERS, json!({}))).await;
    assert!(
        !mine["myServers"]
            .as_array()
            .expect("servers")
            .contains(&json!({ "id": s })),
        "{mine}"
    );
    let bob_id = roles.bob.user_id.to_string();
    assert!(
        !member_ids(&schema, &roles.alice, &s)
            .await
            .contains(&bob_id)
    );
    let bob_uuid: uuid::Uuid = roles.bob.user_id.into();
    let memberships = entity::conversation_member::Entity::find()
        .filter(entity::conversation_member::Column::UserId.eq(bob_uuid))
        .all(&ctx.conn)
        .await
        .expect("memberships");
    assert!(memberships.is_empty(), "{memberships:?}");
    let kept = entity::message::Entity::find()
        .filter(entity::message::Column::Text.eq("still here"))
        .all(&ctx.conn)
        .await
        .expect("messages");
    assert_eq!(kept.len(), 1);

    let data = exec_ok(&schema, roles.gina.request(LEAVE, json!({ "s": s }))).await;
    assert_eq!(data, json!({ "leaveWorkspace": true }));
    assert_eq!(
        exec_err_code(&schema, roles.bob.request(LEAVE, json!({ "s": s }))).await,
        "FORBIDDEN"
    );

    let gina_uuid: uuid::Uuid = roles.gina.user_id.into();
    assert_eq!(
        departures(&events.events()),
        vec![
            (bob_uuid, Some("left".to_string())),
            (gina_uuid, Some("left".to_string())),
        ]
    );
    assert_eq!(
        audit_rows(&ctx, &s, "workspace.leave").await,
        vec![
            (bob_uuid, "workspace".to_string()),
            (gina_uuid, "workspace".to_string()),
        ]
    );
}

/// Nobody leaves the server's default workspace.
#[tokio::test]
async fn nobody_leaves_the_default_workspace() {
    let ctx = TestCtx::new().await;
    let roles = ctx.roles("default").await;
    let workspace = roles.server_id.parse::<uuid::Uuid>().expect("server id");
    let schema =
        ctx.build_schema_with_default_workspace(workspace.into(), RegistrationMode::InviteOnly);
    let s = roles.server_id.clone();

    let (code, message) = exec_err(&schema, roles.bob.request(LEAVE, json!({ "s": s }))).await;
    assert_eq!(code, "VALIDATION");
    assert!(message.contains("default workspace"), "{message}");
    assert!(
        member_ids(&schema, &roles.alice, &s)
            .await
            .contains(&roles.bob.user_id.to_string())
    );
    assert!(audit_rows(&ctx, &s, "workspace.leave").await.is_empty());
}

/// Only the owner renames the workspace, to a valid name they do not already use elsewhere;
/// its members see the new name and are told with `WorkspaceUpdated`.
#[tokio::test]
async fn the_owner_renames_the_workspace_and_its_members_see_it() {
    let ctx = TestCtx::new().await;
    let roles = ctx.roles("rename").await;
    let (schema, _provider, events) = ctx.build_recording(RegistrationMode::InviteOnly);
    let s = roles.server_id.clone();
    let to = |name: &str| json!({ "s": s, "n": name });

    for who in [&roles.mona, &roles.bob, &roles.gina] {
        assert_eq!(
            exec_err_code(&schema, who.request(RENAME, to("X"))).await,
            "FORBIDDEN"
        );
    }
    for name in ["   ".to_string(), "n".repeat(129)] {
        assert_eq!(
            exec_err_code(&schema, roles.alice.request(RENAME, to(&name))).await,
            "VALIDATION"
        );
    }
    exec_ok(
        &schema,
        roles
            .alice
            .request(CREATE_SERVER, json!({ "name": "Design" })),
    )
    .await;
    let (code, message) = exec_err(&schema, roles.alice.request(RENAME, to(" design "))).await;
    assert_eq!(code, "CONFLICT");
    assert_eq!(message, "you already own a workspace named \"Design\"");

    let data = exec_ok(&schema, roles.alice.request(RENAME, to("  Renamed  "))).await;
    assert_eq!(
        data["renameWorkspace"],
        json!({ "id": s, "name": "Renamed" })
    );
    let mine = exec_ok(&schema, roles.bob.request(MY_SERVER_NAMES, json!({}))).await;
    assert!(
        mine["myServers"]
            .as_array()
            .expect("servers")
            .contains(&json!({ "id": s, "name": "Renamed" })),
        "{mine}"
    );
    let data = exec_ok(&schema, roles.alice.request(RENAME, to("renamed"))).await;
    assert_eq!(data["renameWorkspace"]["name"], json!("renamed"));

    let renamed: Vec<String> = events
        .events()
        .into_iter()
        .filter_map(|e| match e {
            WorkspaceEvent::WorkspaceUpdated { name, .. } => Some(name),
            _ => None,
        })
        .collect();
    assert_eq!(renamed, ["Renamed", "renamed"]);
    let alice_uuid: uuid::Uuid = roles.alice.user_id.into();
    assert_eq!(
        audit_rows(&ctx, &s, "workspace.rename").await,
        vec![
            (alice_uuid, "workspace".to_string()),
            (alice_uuid, "workspace".to_string()),
        ]
    );
}

/// The direct space can be neither left nor renamed.
#[tokio::test]
async fn the_direct_space_is_neither_left_nor_renamed() {
    let ctx = TestCtx::new().await;
    let direct = ctx.create_server("direct").await;
    let bob = ctx.create_user("direct-bob").await;
    ctx.add_member(
        &direct,
        &bob,
        rim_core::server_member::model::ServerMemberRole::Guest,
    )
    .await;
    let bob = ctx.caller(&bob).await;
    let (schema, _provider) = ctx.build_with_direct_space(direct.id);
    let s = direct.id.to_string();

    for (document, vars) in [
        (LEAVE, json!({ "s": s })),
        (RENAME, json!({ "s": s, "n": "Mine" })),
    ] {
        let (code, message) = exec_err(&schema, bob.request(document, vars)).await;
        assert_eq!(code, "VALIDATION");
        assert!(
            message.contains("not available in the direct space"),
            "{message}"
        );
    }
}

/// The schema names both mutations.
#[tokio::test]
async fn the_sdl_names_leave_and_rename() {
    let ctx = TestCtx::new().await;
    let sdl = ctx.build_schema(RegistrationMode::InviteOnly).await.sdl();
    assert!(sdl.contains("leaveWorkspace(serverId: ID!): Boolean!"));
    assert!(sdl.contains("renameWorkspace(serverId: ID!, name: String!): Server!"));
}
