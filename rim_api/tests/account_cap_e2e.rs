//! The account cap (`RIM_LIMIT_ACCOUNTS`): every signup path, password and key-only, with an
//! instance invite, a workspace invite, in open mode, a remote user signing up here and a
//! proof-of-work, is refused with `REGISTRATION_FULL` once the server holds as many local
//! accounts as the cap allows, leaves nothing behind, and goes through with one place more.
//! Accounts waiting out their deletion grace period count; remote users and tombstones do not.
//! `serverInfo.registrationFull`, `serverInfo.limits.accounts` and the discovery document's
//! `registration.full` say so before anyone tries.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use async_graphql::Request;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};

use rim_api::auth_context::RemoteIp;
use rim_core::host::model::NewRemoteUser;
use rim_core::host::repository::HostRepository;
use rim_core::user::repository::UserRepository;
use rim_identity::testing::TestIdentity;
use rim_identity::{KeyState, StatementVersion};
use rim_infra_db::sea_orm_impl::host::SeaOrmHostRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_services::auth::dto::RegistrationMode;
use rim_services::limits::InstanceLimits;

use support::TestCtx;
use support::graphql::{anonymous_request, exec_err, exec_ok};
use support::identity::{SIGNUP_WITH_KEY, signup_input};
use support::nodes::{Node, databases, start};
use support::registration::{seed_operator_invite, seed_workspace_invite};
use support::relay::{key_signup, keys};

const SIGNUP_WITH_INVITE: &str = r#"
mutation Signup($input: SignupWithInviteInput!) { signupWithInvite(input: $input) { userId } }"#;

const REGISTRATION_FULL: &str = "{ serverInfo { registrationFull } }";

/// The ways an account comes to exist here, each refused at the cap.
#[derive(Debug, Clone, Copy)]
enum Path {
    PasswordInstanceInvite,
    PasswordWorkspaceInvite,
    PasswordOpen,
    KeyInstanceInvite,
    KeyOpen,
    KeyAdoptsRemote,
}

impl Path {
    fn mode(self) -> RegistrationMode {
        match self {
            Self::PasswordInstanceInvite
            | Self::PasswordWorkspaceInvite
            | Self::KeyInstanceInvite => RegistrationMode::InviteOnly,
            Self::PasswordOpen | Self::KeyOpen | Self::KeyAdoptsRemote => RegistrationMode::Open,
        }
    }

    /// The identity seed of a key-only path; its device's seed is ten more.
    fn seed(self) -> u8 {
        match self {
            Self::KeyInstanceInvite => 31,
            Self::KeyOpen => 32,
            _ => 33,
        }
    }

    fn identity(self) -> TestIdentity {
        TestIdentity::with_version(self.seed(), StatementVersion::V2)
    }

    /// The signup this path makes on `schema`, with a fresh proof for a key-only one.
    async fn request(self, schema: &rim_api::AppSchema) -> Request {
        let password = |username: &str, token: Option<(&str, &str)>| {
            let mut input = json!({
                "username": username,
                "email": format!("{username}@example.com"),
                "password": "ValidPass123!",
            });
            if let Some((field, raw)) = token {
                input[field] = json!(raw);
            }
            anonymous_request(SIGNUP_WITH_INVITE, json!({ "input": input }))
                .data(RemoteIp::new("198.51.100.7"))
        };
        let key = |username: &'static str, extra: Value| async move {
            let input =
                signup_input(schema, username, &self.identity(), self.seed() + 10, extra).await;
            anonymous_request(SIGNUP_WITH_KEY, json!({ "input": input }))
                .data(RemoteIp::new("198.51.100.8"))
        };
        match self {
            Self::PasswordInstanceInvite => password(
                "cap_instance",
                Some(("instanceInviteToken", "rim_cap_instance")),
            ),
            Self::PasswordWorkspaceInvite => password(
                "cap_workspace",
                Some(("workspaceInviteToken", "rim_cap_workspace")),
            ),
            Self::PasswordOpen => password("cap_open", None),
            Self::KeyInstanceInvite => {
                key(
                    "cap_key_invite",
                    json!({ "instanceInviteToken": "rim_cap_key_instance" }),
                )
                .await
            }
            Self::KeyOpen => key("cap_key_open", json!({})).await,
            Self::KeyAdoptsRemote => key("cap_adopted", json!({})).await,
        }
    }
}

async fn local_accounts(ctx: &TestCtx) -> u32 {
    let n = SeaOrmUserRepository::new(ctx.db.clone())
        .count_local_accounts(1000)
        .await
        .expect("count");
    u32::try_from(n).expect("small")
}

async fn registration_full(schema: &rim_api::AppSchema) -> bool {
    exec_ok(schema, anonymous_request(REGISTRATION_FULL, json!({}))).await["serverInfo"]
        ["registrationFull"]
        .as_bool()
        .expect("bool")
}

/// Run `sql` against the test database.
async fn sql(ctx: &TestCtx, sql: &str, values: Vec<sea_orm::Value>) {
    ctx.conn
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await
        .expect("sql");
}

#[tokio::test]
async fn every_signup_path_is_refused_at_the_cap_and_admitted_with_one_place_more() {
    let ctx = TestCtx::new().await;
    let owner = ctx.create_user("cap_owner").await;
    let server = ctx.create_server("cap_ws").await;
    ctx.add_owner(&server, &owner).await;
    let leaving = ctx.create_user("cap_leaving").await;
    let gone = ctx.create_user("cap_gone").await;
    sql(
        &ctx,
        r#"UPDATE "user" SET deletion_requested_at = now(),
               deletion_scheduled_for = now() + interval '14 days' WHERE id = $1"#,
        vec![uuid::Uuid::from(leaving.id).into()],
    )
    .await;
    sql(
        &ctx,
        r#"UPDATE "user" SET deleted_at = now(), username = 'deleted-0000cap', email = NULL,
               password_hash = NULL, principal_id = 'rimp1_cap_gone' WHERE id = $1"#,
        vec![uuid::Uuid::from(gone.id).into()],
    )
    .await;
    let remote = Path::KeyAdoptsRemote.identity();
    let remote_state = KeyState::from_inception(&remote.inception()).expect("inception");
    let remote_id = SeaOrmHostRepository::new(ctx.db.clone())
        .create_remote_user(NewRemoteUser {
            principal_id: remote_state.principal_id.clone(),
            identity_key: remote_state.identity_key.to_string(),
            username: "r_cap_remote".to_string(),
            display_name: "Remote".to_string(),
            home_relay: Some("https://relay.example".to_string()),
        })
        .await
        .expect("remote user");
    let expires = Utc::now() + chrono::Duration::hours(1);
    seed_operator_invite(&ctx.db, "rim_cap_instance", expires).await;
    seed_operator_invite(&ctx.db, "rim_cap_key_instance", expires).await;
    seed_workspace_invite(&ctx.db, owner.id, server.id, "rim_cap_workspace").await;

    assert_eq!(
        local_accounts(&ctx).await,
        2,
        "the owner and the account in its grace period; no remote user, no tombstone"
    );

    for path in [
        Path::PasswordInstanceInvite,
        Path::PasswordWorkspaceInvite,
        Path::PasswordOpen,
        Path::KeyInstanceInvite,
        Path::KeyOpen,
        Path::KeyAdoptsRemote,
    ] {
        let count = local_accounts(&ctx).await;
        let capped = |accounts| {
            ctx.build_with_limits_in(
                InstanceLimits {
                    accounts: Some(accounts),
                    ..Default::default()
                },
                path.mode(),
            )
            .0
        };

        let full = capped(count);
        assert!(registration_full(&full).await, "{path:?}");
        let (code, message) = exec_err(&full, path.request(&full).await).await;
        assert_eq!(code, "REGISTRATION_FULL", "{path:?}: {message}");
        assert_eq!(
            message,
            "this server is full and takes no new accounts; run your own server or sign up on \
             another one"
        );
        assert_eq!(local_accounts(&ctx).await, count, "{path:?} left nothing");

        // The refused signup spent no invite: the same one goes through with a place free.
        let room = capped(count + 1);
        assert!(!registration_full(&room).await, "{path:?}");
        exec_ok(&room, path.request(&room).await).await;
        assert_eq!(local_accounts(&ctx).await, count + 1, "{path:?}");
        assert!(
            registration_full(&room).await,
            "{path:?} filled the last place"
        );
    }

    let adopted = SeaOrmUserRepository::new(ctx.db.clone())
        .find_by_username("cap_adopted")
        .await
        .expect("lookup")
        .expect("the remote user is local now");
    assert_eq!(adopted.id, remote_id);
}

/// The discovery document of `node`.
async fn discovery(http: &reqwest::Client, node: &Node) -> Value {
    http.get(format!("{}/.well-known/rim-server", node.origin))
        .send()
        .await
        .expect("discovery")
        .json()
        .await
        .expect("json")
}

/// `serverInfo { registrationFull limits { accounts } }` on `node`.
async fn server_info(http: &reqwest::Client, node: &Node) -> Value {
    let body: Value = http
        .post(format!("{}/api", node.origin))
        .json(&json!({ "query": "{ serverInfo { registrationFull limits { accounts } } }" }))
        .send()
        .await
        .expect("serverInfo")
        .json()
        .await
        .expect("json");
    body["data"]["serverInfo"].clone()
}

/// A proof-of-work key signup of `name` with the seed's identity; the GraphQL answer.
async fn pow_signup(http: &reqwest::Client, node: &Node, name: &str, seed: u8, bits: u32) -> Value {
    let person = keys(seed);
    let inception = person.identity.inception().payload;
    let nonce = rim_s2s::pow::solve_pow(&node.id(), &inception, bits);
    let extra = json!({ "proofOfWork": URL_SAFE_NO_PAD.encode(nonce) });
    key_signup(http, node, name, &person, extra).await.0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_relay_says_so_and_refuses_a_proof_of_work_signup() {
    const BITS: u32 = 8;
    let (first, _) = databases().await;
    let node = start(&first, |c| {
        c.registration_pow_bits = Some(BITS);
        c.limit_accounts = Some(1);
    })
    .await;
    let http = reqwest::Client::new();

    assert_eq!(
        discovery(&http, &node).await["registration"],
        json!({ "full": false })
    );
    assert_eq!(
        server_info(&http, &node).await,
        json!({ "registrationFull": false, "limits": { "accounts": 1 } })
    );

    let body = pow_signup(&http, &node, "ada", 41, BITS).await;
    assert!(body.get("errors").is_none(), "{body}");
    assert_eq!(
        discovery(&http, &node).await["registration"],
        json!({ "full": true })
    );
    assert_eq!(
        server_info(&http, &node).await["registrationFull"],
        json!(true)
    );

    let body = pow_signup(&http, &node, "bob", 42, BITS).await;
    assert_eq!(
        body["errors"][0]["extensions"]["code"],
        json!("REGISTRATION_FULL"),
        "{body}"
    );
    node.stop().await;
}

#[tokio::test]
async fn a_full_instance_refuses_invalid_signup_inputs_before_validation() {
    let ctx = TestCtx::new().await;
    ctx.create_user("cap_existing").await;
    for mode in [RegistrationMode::Open, RegistrationMode::InviteOnly] {
        let schema = ctx
            .build_with_limits_in(
                InstanceLimits {
                    accounts: Some(1),
                    ..Default::default()
                },
                mode,
            )
            .0;
        for tokens in [
            json!({}),
            json!({"instanceInviteToken":"unknown"}),
            json!({"instanceInviteToken":"unknown", "workspaceInviteToken":"unknown"}),
        ] {
            let mut input = json!({"username":"!", "email":"invalid", "password":"",
                "acceptedTermsUrl":"https://wrong.example/terms"});
            input
                .as_object_mut()
                .expect("input")
                .extend(tokens.as_object().expect("tokens").clone());
            let (code, _) = exec_err(
                &schema,
                anonymous_request(SIGNUP_WITH_INVITE, json!({"input":input})),
            )
            .await;
            assert_eq!(code, "REGISTRATION_FULL");
            let identity = TestIdentity::with_version(91, StatementVersion::V2);
            let mut input = signup_input(&schema, "!", &identity, 92, tokens).await;
            input["keyLog"] = json!([]);
            input["acceptedTermsUrl"] = json!("https://wrong.example/terms");
            let (code, _) = exec_err(
                &schema,
                anonymous_request(SIGNUP_WITH_KEY, json!({"input":input})),
            )
            .await;
            assert_eq!(code, "REGISTRATION_FULL");
        }
    }
    assert_eq!(local_accounts(&ctx).await, 1);
}
