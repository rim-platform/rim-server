//! Track 0.4 e2e tests: per-workspace feature flags over the real GraphQL resolvers, real
//! services and a real database (`docs/specs/track-0-4-optional-incident-module.md`).
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use async_graphql::{Request, Response, Variables};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};

use rim_api::auth_context::AuthActorCtx;
use rim_core::auth::model::SessionId;
use rim_core::server::model::Server;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::User;
use rim_core::workspace_feature::model::WorkspaceFeature;
use rim_services::alert::{self, AlertSource};
use rim_services::auth::dto::RegistrationMode;
use rim_services::incident::command::IngestAlertCmd;
use rim_services::{Actor, ServiceError};

use support::TestCtx;

async fn run(
    schema: &rim_api::AppSchema,
    user: &User,
    query: &str,
    vars: serde_json::Value,
) -> Response {
    schema
        .execute(
            Request::new(query)
                .variables(Variables::from_json(vars))
                .data(AuthActorCtx::new(Some(user.id), Some(SessionId::new()))),
        )
        .await
}

fn data(res: Response) -> serde_json::Value {
    assert!(res.errors.is_empty(), "unexpected errors: {:?}", res.errors);
    res.data
        .into_json()
        .expect("data must be JSON-serialisable")
}

fn error_code(res: &Response) -> String {
    let err = res.errors.first().expect("expected a GraphQL error");
    let ext = err
        .extensions
        .as_ref()
        .expect("error must carry extensions");
    match ext.get("code") {
        Some(async_graphql::Value::String(code)) => code.clone(),
        other => panic!("error has no string code: {other:?}"),
    }
}

const SET_FEATURE: &str = r#"
mutation($serverId: ID!, $enabled: Boolean!) {
  setWorkspaceFeature(serverId: $serverId, feature: INCIDENTS, enabled: $enabled) {
    feature enabled updatedAt
  }
}"#;

const LIST_FEATURES: &str = r#"
query($serverId: ID!) {
  workspaceFeatures(serverId: $serverId) { feature enabled updatedAt }
}"#;

async fn set_incidents(
    schema: &rim_api::AppSchema,
    owner: &User,
    server: &Server,
    enabled: bool,
) -> serde_json::Value {
    data(
        run(
            schema,
            owner,
            SET_FEATURE,
            serde_json::json!({ "serverId": server.id.to_string(), "enabled": enabled }),
        )
        .await,
    )
}

/// Owner + workspace + one channel the owner belongs to.
async fn seed(ctx: &TestCtx, schema: &rim_api::AppSchema) -> (User, Server, String) {
    let owner = ctx.create_user("owner").await;
    let server = ctx.create_server("ws").await;
    ctx.add_owner(&server, &owner).await;
    let res = run(
        schema,
        &owner,
        r#"mutation($serverId: ID!) {
          createConversation(input: { serverId: $serverId, isChannel: true, title: "ops" }) { id }
        }"#,
        serde_json::json!({ "serverId": server.id.to_string() }),
    )
    .await;
    let channel_id = data(res)["createConversation"]["id"]
        .as_str()
        .expect("channel id")
        .to_string();
    (owner, server, channel_id)
}

#[tokio::test]
async fn set_and_list_workspace_features_round_trip_with_audit_entry() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let owner = ctx.create_user("owner").await;
    let server = ctx.create_server("ws").await;
    ctx.add_owner(&server, &owner).await;
    let vars = serde_json::json!({ "serverId": server.id.to_string() });

    // A workspace created after the migration has no row: the default (off) is reported.
    let listed = data(run(&schema, &owner, LIST_FEATURES, vars.clone()).await);
    assert_eq!(
        listed["workspaceFeatures"],
        serde_json::json!([{ "feature": "INCIDENTS", "enabled": false, "updatedAt": null }])
    );

    let set = set_incidents(&schema, &owner, &server, true).await;
    assert_eq!(set["setWorkspaceFeature"]["feature"], "INCIDENTS");
    assert_eq!(set["setWorkspaceFeature"]["enabled"], true);
    assert!(set["setWorkspaceFeature"]["updatedAt"].is_string());

    let listed = data(run(&schema, &owner, LIST_FEATURES, vars.clone()).await);
    assert_eq!(listed["workspaceFeatures"][0]["enabled"], true);

    // `Server.features` resolves the same state.
    let servers = data(
        run(
            &schema,
            &owner,
            "query { myServers { id features { feature enabled } } }",
            serde_json::json!({}),
        )
        .await,
    );
    assert_eq!(
        servers["myServers"][0]["features"],
        serde_json::json!([{ "feature": "INCIDENTS", "enabled": true }])
    );

    // The toggle is audited with actor, workspace, feature and new value.
    let server_uuid: uuid::Uuid = server.id.into();
    let owner_uuid: uuid::Uuid = owner.id.into();
    let rows = entity::command_audit_log::Entity::find()
        .filter(entity::command_audit_log::Column::ServerId.eq(server_uuid))
        .filter(entity::command_audit_log::Column::CommandId.eq("workspace.set_feature"))
        .all(&ctx.conn)
        .await
        .expect("audit query");
    assert_eq!(rows.len(), 1, "exactly one audit row per toggle");
    assert_eq!(rows[0].actor_user_id, owner_uuid);
    assert_eq!(
        rows[0].args_redacted,
        serde_json::json!({ "feature": "incidents", "enabled": true })
    );
}

#[tokio::test]
async fn set_workspace_feature_is_owner_only() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let owner = ctx.create_user("owner").await;
    let moderator = ctx.create_user("moderator").await;
    let outsider = ctx.create_user("outsider").await;
    let server = ctx.create_server("ws").await;
    ctx.add_owner(&server, &owner).await;
    ctx.add_member(&server, &moderator, ServerMemberRole::Moderator)
        .await;
    let vars = serde_json::json!({ "serverId": server.id.to_string(), "enabled": true });

    let res = run(&schema, &moderator, SET_FEATURE, vars.clone()).await;
    assert_eq!(error_code(&res), "FORBIDDEN");

    let res = run(&schema, &outsider, SET_FEATURE, vars).await;
    assert_eq!(error_code(&res), "NOT_FOUND");

    // A moderator may still read the flags; an outsider may not.
    let list_vars = serde_json::json!({ "serverId": server.id.to_string() });
    let listed = data(run(&schema, &moderator, LIST_FEATURES, list_vars.clone()).await);
    assert_eq!(listed["workspaceFeatures"][0]["enabled"], false);
    let res = run(&schema, &outsider, LIST_FEATURES, list_vars).await;
    assert_eq!(error_code(&res), "NOT_FOUND");
}

#[tokio::test]
async fn incident_ack_via_execute_command_respects_the_feature_flag() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let (owner, server, channel_id) = seed(&ctx, &schema).await;

    // Starting an incident is itself gated: off by default.
    let start_vars = serde_json::json!({
        "serverId": server.id.to_string(),
        "conversationId": channel_id,
    });
    const START: &str = r#"
    mutation($serverId: ID!, $conversationId: ID!) {
      startIncident(input: { serverId: $serverId, conversationId: $conversationId, title: "db down" }) { id }
    }"#;
    let res = run(&schema, &owner, START, start_vars.clone()).await;
    assert_eq!(error_code(&res), "FEATURE_DISABLED");

    set_incidents(&schema, &owner, &server, true).await;
    let incident_id = data(run(&schema, &owner, START, start_vars).await)["startIncident"]["id"]
        .as_str()
        .expect("incident id")
        .to_string();

    const EXEC: &str = r#"
    mutation($serverId: ID!, $args: JSON!) {
      executeCommand(input: {
        commandId: "incident.ack"
        args: $args
        commandContext: { workspaceId: $serverId }
      }) { status message }
    }"#;
    let exec_vars = serde_json::json!({
        "serverId": server.id.to_string(),
        "args": { "incident_id": incident_id },
    });

    set_incidents(&schema, &owner, &server, false).await;
    let res = run(&schema, &owner, EXEC, exec_vars.clone()).await;
    assert_eq!(error_code(&res), "FEATURE_DISABLED");
    let ext = res.errors[0].extensions.as_ref().expect("extensions");
    assert_eq!(
        ext.get("feature"),
        Some(&async_graphql::Value::from("incidents"))
    );

    // Turning the feature back on restores the incident and the command.
    set_incidents(&schema, &owner, &server, true).await;
    let ok = data(run(&schema, &owner, EXEC, exec_vars).await);
    assert_eq!(ok["executeCommand"]["status"], "SUCCESS");
}

#[tokio::test]
async fn commands_query_omits_incident_commands_when_feature_off() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let owner = ctx.create_user("owner").await;
    let server = ctx.create_server("ws").await;
    ctx.add_owner(&server, &owner).await;

    const COMMANDS: &str = r#"
    query($serverId: ID!) {
      commands(surface: PALETTE, commandContext: { workspaceId: $serverId }) { id feature }
    }"#;
    let vars = serde_json::json!({ "serverId": server.id.to_string() });
    let gated = ["incident.ack", "incident.escalate", "oncall.take_shift"];

    let off = data(run(&schema, &owner, COMMANDS, vars.clone()).await);
    let off = off["commands"].as_array().expect("commands array").clone();
    assert!(!off.is_empty());
    for c in &off {
        let id = c["id"].as_str().unwrap_or_default();
        assert!(!gated.contains(&id), "{id} must be hidden when off");
        assert!(
            c["feature"].is_null(),
            "{id} listed with a feature while off"
        );
    }

    set_incidents(&schema, &owner, &server, true).await;
    let on = data(run(&schema, &owner, COMMANDS, vars).await);
    let on = on["commands"].as_array().expect("commands array").clone();
    for id in gated {
        let cmd = on
            .iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("{id} must be listed when on"));
        assert_eq!(cmd["feature"], "INCIDENTS");
    }
}

/// The alert webhook handler lives in the `rim_server` binary (no library target, so it cannot
/// be driven from here). Its decision inputs are pinned against the real database instead:
/// with the feature off the token still authenticates (so the handler's 404 must come from the
/// feature gate, not a 401) and the feature service reports the workspace disabled, and the
/// service-level gate on `ingest_alert` refuses the alert. The HTTP mapping (bare 404, empty
/// body, rate limiter first) is covered by the handler tests in
/// `rim_server/src/alert_handlers.rs`.
#[tokio::test]
async fn alert_ingest_is_refused_when_feature_off() {
    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let provider = ctx.build_provider();
    let (owner, server, channel_id) = seed(&ctx, &schema).await;

    const CREATE_TOKEN: &str = r#"
    mutation($serverId: ID!, $conversationId: ID!) {
      createIngestToken(input: { serverId: $serverId, conversationId: $conversationId, label: "prom" }) {
        rawToken
      }
    }"#;
    let token_vars = serde_json::json!({
        "serverId": server.id.to_string(),
        "conversationId": channel_id,
    });

    // Minting a token needs the feature.
    let res = run(&schema, &owner, CREATE_TOKEN, token_vars.clone()).await;
    assert_eq!(error_code(&res), "FEATURE_DISABLED");

    set_incidents(&schema, &owner, &server, true).await;
    let raw_token = data(run(&schema, &owner, CREATE_TOKEN, token_vars).await)["createIngestToken"]
        ["rawToken"]
        .as_str()
        .expect("raw token")
        .to_string();
    set_incidents(&schema, &owner, &server, false).await;

    let token = provider
        .ingest_token_service()
        .authenticate_token(&raw_token)
        .await
        .expect("token stays valid while the feature is off");
    assert_eq!(token.server_id, server.id);
    assert!(
        !provider
            .workspace_feature_service()
            .is_enabled(token.server_id, WorkspaceFeature::Incidents)
            .await
            .expect("is_enabled"),
        "the webhook gate must see the workspace as disabled"
    );

    let event = alert::parse_generic(br#"{"title": "disk full"}"#)
        .expect("parse")
        .remove(0);
    let err = provider
        .incident_service()
        .ingest_alert(IngestAlertCmd {
            actor: Actor::new(token.created_by),
            server_id: token.server_id,
            conversation_id: token.conversation_id,
            source: AlertSource::Generic,
            event,
        })
        .await
        .expect_err("ingest must be refused");
    assert!(
        matches!(&err, ServiceError::FeatureDisabled { feature } if feature == "incidents"),
        "got {err:?}"
    );

    let incidents = entity::incident::Entity::find()
        .filter(entity::incident::Column::ServerId.eq(uuid::Uuid::from(server.id)))
        .count(&ctx.conn)
        .await
        .expect("count incidents");
    assert_eq!(
        incidents, 0,
        "nothing may be ingested while the feature is off"
    );
}
