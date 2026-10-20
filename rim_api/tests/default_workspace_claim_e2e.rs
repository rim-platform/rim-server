//! Through-the-stack e2e tests for the default workspace a definition file describes: the start
//! writes a single-use owner claim link, the signup through it creates the workspace with the
//! new account as Owner, and every later start applies the file again.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use sea_orm::{ConnectionTrait, Statement};
use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::server::model::ServerId;
use rim_identity::StatementVersion;
use rim_identity::testing::TestIdentity;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::default_workspace::SeaOrmDefaultWorkspaceRepository;
use rim_infra_db::sea_orm_impl::dm_policy::SeaOrmDmPolicyRepository;
use rim_infra_db::sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_services::default_workspace::{
    DefaultWorkspace, DefinedWorkspace, StartContext, StartRepos, Started, WorkspaceDefinition,
    start,
};
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{anonymous_request, exec_err_code, exec_ok};
use support::identity::{SIGNUP_WITH_KEY, signed_out, signup_input};
use support::registration::seed_operator_invite;

const SIGNUP_WITH_INVITE: &str = r#"
mutation Signup($input: SignupWithInviteInput!) { signupWithInvite(input: $input) { userId serverId } }"#;

const SERVER_INFO: &str = "{ serverInfo { defaultWorkspace } }";

const CREATE_CHANNEL: &str = r#"
mutation Create($input: CreateConversationInput!) { createConversation(input: $input) { id } }"#;

const LINK_BASE: &str = "https://rim.example.org";

const DEFINITION: &str = r#"{
  "name": "RIM",
  "claimGrantsOperator": true,
  "dm": { "outsideFloor": "REQUESTS", "memberDefault": "DIRECT" },
  "channels": [
    { "name": "general", "topic": "Anything about RIM" },
    { "name": "help", "topic": "Questions" },
    { "name": "staff", "private": true }
  ]
}"#;

/// The definition in `text` with its claim link file in a fresh directory.
fn defined(text: &str) -> DefinedWorkspace {
    let dir = std::env::temp_dir().join(format!("rim-claim-e2e-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("claim link dir");
    DefinedWorkspace {
        definition: WorkspaceDefinition::parse(text).expect("definition"),
        claim_link_file: dir.join("owner-claim.txt"),
    }
}

/// Run the start against the test database, as the server does before it serves.
async fn run_start(
    ctx: &TestCtx,
    setting: &DefaultWorkspace,
    signups_take_tokens: bool,
) -> Result<Started, String> {
    let repos = StartRepos {
        default_workspace_repo: Arc::new(SeaOrmDefaultWorkspaceRepository::new(ctx.db.clone())),
        instance_invite_repo: Arc::new(SeaOrmInstanceInviteRepository::new(ctx.db.clone())),
        server_repo: Arc::new(SeaOrmServerRepository::new(ctx.db.clone())),
        conversation_repo: Arc::new(SeaOrmConversationRepository::new(ctx.db.clone())),
        dm_policy_repo: Arc::new(SeaOrmDmPolicyRepository::new(ctx.db.clone())),
    };
    start(
        &repos,
        setting,
        StartContext {
            direct_space_id: ServerId::new(),
            signups_take_tokens,
            link_base: LINK_BASE,
            now: Utc::now(),
        },
    )
    .await
}

/// The token of the claim link in `file`, after checking the link's shape and the file's mode.
fn claim_token(file: &Path) -> String {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(file)
        .expect("claim link file")
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "the claim link file is the owner's alone"
    );
    let link = std::fs::read_to_string(file).expect("read the claim link");
    let token = link
        .trim()
        .strip_prefix(&format!("{LINK_BASE}/join/"))
        .unwrap_or_else(|| panic!("an invite link on the link base, got {link}"));
    assert!(token.starts_with("rim_iinv_"), "{token}");
    token.to_string()
}

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

async fn rows(ctx: &TestCtx, sql: &str, values: Vec<sea_orm::Value>) -> Vec<sea_orm::QueryResult> {
    ctx.conn
        .query_all_raw(Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            sql,
            values,
        ))
        .await
        .expect("query")
}

/// Every workspace on the server, by name.
async fn workspaces(ctx: &TestCtx) -> Vec<(String, String)> {
    rows(
        ctx,
        "SELECT id::text AS id, name FROM server WHERE kind = 'workspace' ORDER BY name",
        vec![],
    )
    .await
    .iter()
    .map(|r| {
        (
            r.try_get::<String>("", "id").expect("id"),
            r.try_get::<String>("", "name").expect("name"),
        )
    })
    .collect()
}

/// The channels of `workspace`: title, topic and privacy, by title.
async fn channels(ctx: &TestCtx, workspace: &str) -> Vec<(String, Option<String>, bool)> {
    rows(
        ctx,
        "SELECT title, topic, is_private FROM conversation \
         WHERE server_id = $1::uuid AND conversation_type = 'channel' ORDER BY title",
        vec![workspace.into()],
    )
    .await
    .iter()
    .map(|r| {
        (
            r.try_get::<String>("", "title").expect("title"),
            r.try_get::<Option<String>>("", "topic").expect("topic"),
            r.try_get::<bool>("", "is_private").expect("is_private"),
        )
    })
    .collect()
}

/// The role of `user_id` in `workspace`, `None` when not a member.
async fn role(ctx: &TestCtx, workspace: &str, user_id: &str) -> Option<String> {
    rows(
        ctx,
        "SELECT member_role::text AS role FROM server_member \
         WHERE server_id = $1::uuid AND user_id = $2::uuid",
        vec![workspace.into(), user_id.into()],
    )
    .await
    .first()
    .map(|r| r.try_get::<String>("", "role").expect("role"))
}

/// The titles of the channels `user_id` is a member of in `workspace`, sorted.
async fn joined(ctx: &TestCtx, workspace: &str, user_id: &str) -> Vec<String> {
    rows(
        ctx,
        "SELECT c.title FROM conversation c \
         JOIN conversation_member m ON m.conversation_id = c.id \
         WHERE c.server_id = $1::uuid AND m.user_id = $2::uuid ORDER BY c.title",
        vec![workspace.into(), user_id.into()],
    )
    .await
    .iter()
    .map(|r| r.try_get::<String>("", "title").expect("title"))
    .collect()
}

async fn dm_policy(ctx: &TestCtx, workspace: &str) -> (String, String) {
    let row = rows(
        ctx,
        "SELECT dm_outside_floor, dm_member_default FROM server WHERE id = $1::uuid",
        vec![workspace.into()],
    )
    .await;
    let row = row.first().expect("the workspace");
    (
        row.try_get::<String>("", "dm_outside_floor")
            .expect("floor"),
        row.try_get::<String>("", "dm_member_default")
            .expect("member default"),
    )
}

async fn is_operator(ctx: &TestCtx, user_id: &str) -> bool {
    rows(
        ctx,
        r#"SELECT instance_operator FROM "user" WHERE id = $1::uuid"#,
        vec![user_id.into()],
    )
    .await
    .first()
    .expect("the account")
    .try_get::<bool>("", "instance_operator")
    .expect("instance_operator")
}

/// A caller acting as the account `user_id`.
async fn caller_for(ctx: &TestCtx, user_id: &str) -> support::graphql::Caller {
    use rim_core::user::repository::UserRepository;

    let id: uuid::Uuid = user_id.parse().expect("uuid userId");
    let user = rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository::new(ctx.db.clone())
        .get_by_id(id.into())
        .await
        .expect("user lookup")
        .expect("the account");
    ctx.caller(&user).await
}

fn id(value: &Value) -> String {
    value.as_str().expect("an id").to_string()
}

fn cleanup(file: &Path) {
    if let Some(dir) = file.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The password signup through the owner claim link creates the workspace from the file, with
/// the new account as Owner and instance operator, its channels, topics and DM policy; adds the
/// account that already existed as a Member; deletes the claim link file; and answers the
/// workspace as `serverId` and in `serverInfo`. The link then admits nobody, and a later signup
/// joins the claimed workspace as a Member with its public channels.
#[tokio::test]
async fn the_claim_creates_the_workspace_and_later_signups_join_it() {
    let ctx = TestCtx::new().await;
    let earlier = ctx.create_user("dwc-earlier").await;
    let defined = defined(DEFINITION);
    let file: PathBuf = defined.claim_link_file.clone();
    let setting = DefaultWorkspace::Defined(Arc::new(defined.clone()));

    let started = run_start(&ctx, &setting, true).await.expect("start");
    assert!(
        matches!(&started, Started::ClaimWaiting { file: f, workspace_deleted: false, .. } if *f == file),
        "{started:?}"
    );
    assert!(
        workspaces(&ctx).await.is_empty(),
        "nothing exists before the claim"
    );
    let token = claim_token(&file);

    let schema: AppSchema =
        ctx.build_schema_with_defined_workspace(defined, RegistrationMode::InviteOnly);
    let info = exec_ok(&schema, anonymous_request(SERVER_INFO, json!({}))).await;
    assert!(info["serverInfo"]["defaultWorkspace"].is_null());

    let data = exec_ok(
        &schema,
        password_signup("dwc-owner", json!({ "instanceInviteToken": token })),
    )
    .await;
    let owner = id(&data["signupWithInvite"]["userId"]);
    let workspace = id(&data["signupWithInvite"]["serverId"]);

    assert_eq!(
        workspaces(&ctx).await,
        vec![(workspace.clone(), "RIM".to_string())]
    );
    assert_eq!(
        channels(&ctx, &workspace).await,
        vec![
            (
                "general".to_string(),
                Some("Anything about RIM".to_string()),
                false
            ),
            ("help".to_string(), Some("Questions".to_string()), false),
            ("staff".to_string(), None, true),
        ]
    );
    assert_eq!(
        dm_policy(&ctx, &workspace).await,
        ("requests".to_string(), "direct".to_string())
    );
    assert_eq!(
        role(&ctx, &workspace, &owner).await.as_deref(),
        Some("owner")
    );
    assert_eq!(
        joined(&ctx, &workspace, &owner).await,
        ["general", "help", "staff"]
    );
    assert!(is_operator(&ctx, &owner).await);
    let earlier = earlier.id.to_string();
    assert_eq!(
        role(&ctx, &workspace, &earlier).await.as_deref(),
        Some("member")
    );
    assert_eq!(
        joined(&ctx, &workspace, &earlier).await,
        ["general", "help"]
    );
    assert!(!is_operator(&ctx, &earlier).await);
    assert!(!file.exists(), "the claim deletes the claim link file");

    let info = exec_ok(&schema, anonymous_request(SERVER_INFO, json!({}))).await;
    assert_eq!(info["serverInfo"]["defaultWorkspace"], json!(workspace));

    let again = exec_err_code(
        &schema,
        password_signup("dwc-second", json!({ "instanceInviteToken": token })),
    )
    .await;
    assert_eq!(again, "INVALID_TOKEN");

    seed_operator_invite(
        &ctx.db,
        "rim_iinv_dwc_later",
        Utc::now() + chrono::Duration::minutes(15),
    )
    .await;
    let data = exec_ok(
        &schema,
        password_signup(
            "dwc-later",
            json!({ "instanceInviteToken": "rim_iinv_dwc_later" }),
        ),
    )
    .await;
    assert!(data["signupWithInvite"]["serverId"].is_null());
    let later = id(&data["signupWithInvite"]["userId"]);
    assert_eq!(
        role(&ctx, &workspace, &later).await.as_deref(),
        Some("member")
    );
    assert_eq!(joined(&ctx, &workspace, &later).await, ["general", "help"]);
    assert!(!is_operator(&ctx, &later).await);
    assert_eq!(workspaces(&ctx).await.len(), 1);
    cleanup(&file);
}

/// In proof-of-work mode the claim link stands in for the proof-of-work of a key signup, and a
/// definition without `claimGrantsOperator` leaves the owner without the operator role.
#[tokio::test]
async fn a_key_signup_claims_with_the_link_in_pow_mode() {
    let ctx = TestCtx::new().await;
    let defined = defined(r#"{ "name": "Company", "channels": [{ "name": "general" }] }"#);
    let file = defined.claim_link_file.clone();
    let setting = DefaultWorkspace::Defined(Arc::new(defined.clone()));
    run_start(&ctx, &setting, true).await.expect("start");
    let token = claim_token(&file);

    let schema =
        ctx.build_schema_with_defined_workspace(defined, RegistrationMode::Pow { bits: 24 });
    let identity = TestIdentity::with_version(62, StatementVersion::V2);
    let input = signup_input(
        &schema,
        "dwc-key",
        &identity,
        72,
        json!({ "instanceInviteToken": token }),
    )
    .await;
    let data = exec_ok(
        &schema,
        signed_out(SIGNUP_WITH_KEY, json!({ "input": input })),
    )
    .await;
    let owner = id(&data["signupWithKey"]["userId"]);
    let workspace = id(&data["signupWithKey"]["serverId"]);

    assert_eq!(
        workspaces(&ctx).await,
        vec![(workspace.clone(), "Company".to_string())]
    );
    assert_eq!(
        role(&ctx, &workspace, &owner).await.as_deref(),
        Some("owner")
    );
    assert_eq!(joined(&ctx, &workspace, &owner).await, ["general"]);
    assert!(!is_operator(&ctx, &owner).await);
    assert!(!file.exists());
    cleanup(&file);
}

/// Each start while the workspace is unclaimed replaces the claim link: the previous token is
/// revoked and admits nobody, the new one claims.
#[tokio::test]
async fn a_restart_while_unclaimed_revokes_the_previous_link() {
    let ctx = TestCtx::new().await;
    let defined = defined(DEFINITION);
    let file = defined.claim_link_file.clone();
    let setting = DefaultWorkspace::Defined(Arc::new(defined.clone()));

    run_start(&ctx, &setting, true).await.expect("first start");
    let first = claim_token(&file);
    run_start(&ctx, &setting, true).await.expect("second start");
    let second = claim_token(&file);
    assert_ne!(first, second);

    let schema = ctx.build_schema_with_defined_workspace(defined, RegistrationMode::InviteOnly);
    let refused = exec_err_code(
        &schema,
        password_signup("dwc-stale", json!({ "instanceInviteToken": first })),
    )
    .await;
    assert_eq!(refused, "INVALID_TOKEN");
    assert!(workspaces(&ctx).await.is_empty());

    let data = exec_ok(
        &schema,
        password_signup("dwc-fresh", json!({ "instanceInviteToken": second })),
    )
    .await;
    assert!(data["signupWithInvite"]["serverId"].is_string());
    assert_eq!(workspaces(&ctx).await.len(), 1);
    cleanup(&file);
}

/// A start after the claim applies the file again: a renamed workspace, a new channel and a
/// changed topic arrive, while a channel made in the app, a topic the file leaves out, the
/// members and their roles stay, and no claim link is written.
#[tokio::test]
async fn a_later_start_adds_and_updates_but_deletes_nothing() {
    let ctx = TestCtx::new().await;
    let defined = defined(DEFINITION);
    let file = defined.claim_link_file.clone();
    let setting = DefaultWorkspace::Defined(Arc::new(defined.clone()));
    run_start(&ctx, &setting, true).await.expect("start");
    let token = claim_token(&file);
    let schema = ctx.build_schema_with_defined_workspace(defined, RegistrationMode::InviteOnly);
    let data = exec_ok(
        &schema,
        password_signup("dwc-keeper", json!({ "instanceInviteToken": token })),
    )
    .await;
    let owner = id(&data["signupWithInvite"]["userId"]);
    let workspace = id(&data["signupWithInvite"]["serverId"]);
    let owner_caller = caller_for(&ctx, &owner).await;
    exec_ok(
        &schema,
        owner_caller.request(
            CREATE_CHANNEL,
            json!({ "input": {
                "serverId": workspace, "isChannel": true, "title": "random", "isPrivate": false,
            } }),
        ),
    )
    .await;
    ctx.conn
        .execute_raw(Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "UPDATE conversation SET topic = 'Staff only' \
             WHERE server_id = $1::uuid AND title = 'staff'",
            vec![workspace.as_str().into()],
        ))
        .await
        .expect("a topic set in the app");

    let changed = defined_with(
        &file,
        r#"{
          "name": "RIM community",
          "dm": { "outsideFloor": "CLOSED", "memberDefault": "REQUESTS" },
          "channels": [
            { "name": "general", "topic": "Anything at all" },
            { "name": "help", "topic": "Questions" },
            { "name": "plugins", "topic": "Writing plugins" }
          ]
        }"#,
    );
    let started = run_start(&ctx, &DefaultWorkspace::Defined(Arc::new(changed)), true)
        .await
        .expect("later start");
    let Started::Applied {
        workspace: applied,
        changes,
    } = started
    else {
        panic!("expected the file applied, got {started:?}");
    };
    assert_eq!(applied.to_string(), workspace);
    assert_eq!(changes.renamed_to.as_deref(), Some("RIM community"));
    assert_eq!(changes.channels_added, ["plugins"]);
    assert_eq!(changes.topics_set, ["general", "plugins"]);
    assert!(changes.dm_policy_set);
    assert!(!file.exists(), "a claimed workspace gets no claim link");

    assert_eq!(
        workspaces(&ctx).await,
        vec![(workspace.clone(), "RIM community".to_string())]
    );
    assert_eq!(
        channels(&ctx, &workspace).await,
        vec![
            (
                "general".to_string(),
                Some("Anything at all".to_string()),
                false
            ),
            ("help".to_string(), Some("Questions".to_string()), false),
            (
                "plugins".to_string(),
                Some("Writing plugins".to_string()),
                false
            ),
            ("random".to_string(), None, false),
            ("staff".to_string(), Some("Staff only".to_string()), true),
        ]
    );
    assert_eq!(
        dm_policy(&ctx, &workspace).await,
        ("closed".to_string(), "requests".to_string())
    );
    assert_eq!(
        role(&ctx, &workspace, &owner).await.as_deref(),
        Some("owner")
    );

    let unchanged = defined_with(&file, r#"{ "name": "RIM community" }"#);
    let started = run_start(&ctx, &DefaultWorkspace::Defined(Arc::new(unchanged)), true)
        .await
        .expect("start with nothing to change");
    assert!(
        matches!(&started, Started::Applied { changes, .. } if *changes == Default::default()),
        "{started:?}"
    );
    cleanup(&file);
}

/// The definition in `text` with the claim link file `file`.
fn defined_with(file: &Path, text: &str) -> DefinedWorkspace {
    DefinedWorkspace {
        definition: WorkspaceDefinition::parse(text).expect("definition"),
        claim_link_file: file.to_path_buf(),
    }
}

/// An open server takes no invite token at signup, so it refuses to start with a workspace
/// waiting for its claim; once claimed, the definition file and open registration go together.
#[tokio::test]
async fn open_registration_cannot_wait_for_a_claim() {
    let ctx = TestCtx::new().await;
    let defined = defined(DEFINITION);
    let file = defined.claim_link_file.clone();
    let setting = DefaultWorkspace::Defined(Arc::new(defined));
    let err = run_start(&ctx, &setting, false).await.unwrap_err();
    assert!(err.contains("RIM_OPEN_REGISTRATION"), "{err}");
    assert!(!file.exists());
    cleanup(&file);
}

/// A server that stops using the definition file revokes the claim link an earlier start
/// issued.
#[tokio::test]
async fn dropping_the_file_revokes_a_waiting_claim() {
    let ctx = TestCtx::new().await;
    let defined = defined(DEFINITION);
    let file = defined.claim_link_file.clone();
    run_start(&ctx, &DefaultWorkspace::Defined(Arc::new(defined)), true)
        .await
        .expect("start with the file");
    let token = claim_token(&file);

    assert_eq!(
        run_start(&ctx, &DefaultWorkspace::None, true).await,
        Ok(Started::None)
    );
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let refused = exec_err_code(
        &schema,
        password_signup("dwc-dropped", json!({ "instanceInviteToken": token })),
    )
    .await;
    assert_eq!(refused, "INVALID_TOKEN");
    cleanup(&file);
}
