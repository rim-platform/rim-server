//! Resolver-level e2e tests for the registration GraphQL endpoints.
//!
//! These tests exercise the *full* GraphQL resolver path:
//! parse input → resolve via context → call into a real `DefaultAuthService`
//! over a real database.  Schema-only unit variants are covered in
//! `rim_api/src/tests.rs` (tests 20–25).
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use async_graphql::Request;
use chrono::Utc;

use rim_core::server_member::model::ServerMemberRole;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::registration::{seed_operator_invite, seed_workspace_invite};

// ── Test 13 ──────────────────────────────────────────────────────────────────

/// Verify that `query { registrationMode }` returns the configured mode.
///
/// Tests both `INVITE_ONLY` (default) and `OPEN` variants by building two
/// schemas with different `RegistrationMode` context values.
#[tokio::test]
async fn query_registration_mode_returns_invite_only_by_default() {
    let ctx = TestCtx::new().await;

    // ── InviteOnly ────────────────────────────────────────────────────────
    let schema_invite_only = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let res = schema_invite_only
        .execute(Request::new("query { registrationMode }"))
        .await;
    assert!(
        res.errors.is_empty(),
        "registrationMode query should have no errors (InviteOnly), got: {:?}",
        res.errors
    );
    let json = res
        .data
        .into_json()
        .expect("data must be JSON-serialisable");
    assert_eq!(
        json["registrationMode"],
        serde_json::json!("INVITE_ONLY"),
        "expected INVITE_ONLY"
    );

    // ── Open ──────────────────────────────────────────────────────────────
    let schema_open = ctx.build_schema(RegistrationMode::Open).await;
    let res = schema_open
        .execute(Request::new("query { registrationMode }"))
        .await;
    assert!(
        res.errors.is_empty(),
        "registrationMode query should have no errors (Open), got: {:?}",
        res.errors
    );
    let json = res
        .data
        .into_json()
        .expect("data must be JSON-serialisable");
    assert_eq!(
        json["registrationMode"],
        serde_json::json!("OPEN"),
        "expected OPEN"
    );
}

// ── Test 14 ──────────────────────────────────────────────────────────────────

/// Verify `previewInviteToken(token)` behaviour for a workspace invite.
///
/// Seeds a user, server, and workspace invite, then:
/// 1. Asserts `tokenType == WORKSPACE` and populated workspace fields, the inviter's and the
///    owner's names included.
/// 2. Asserts the underlying `invite` row's `use_count` is unchanged (preview
///    is read-only — does not consume the token).
/// 3. Asserts `INVALID_TOKEN` for an unknown token.
/// 4. Asserts `RATE_LIMITED` on the 31st request from the same IP within the
///    rate-limit window (30 requests per 5 min per IP).
#[tokio::test]
async fn query_preview_invite_token_workspace_happy_path() {
    let ctx = TestCtx::new().await;

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    // Seed: owner, a moderator who issues the invite, server, workspace invite.
    let owner = ctx.create_user("preview_owner").await;
    let inviter = ctx.create_user("preview_inviter").await;
    let server = ctx.create_server("preview_ws").await;
    ctx.add_owner(&server, &owner).await;
    ctx.add_member(&server, &inviter, ServerMemberRole::Moderator)
        .await;

    let raw_token = "test_raw_preview_ws_token_1234567890abcdef";
    let _invite_id = seed_workspace_invite(&ctx.db, inviter.id, server.id, raw_token).await;

    // ── Happy path: workspace preview ─────────────────────────────────────
    let query = format!(
        r#"query {{
            previewInviteToken(token: "{raw_token}") {{
                tokenType
                expiresAt
                workspace {{
                    serverId
                    serverName
                    invitedByDisplayName
                    ownerDisplayName
                }}
            }}
        }}"#
    );
    let res = schema.execute(Request::new(&query)).await;
    assert!(
        res.errors.is_empty(),
        "previewInviteToken workspace: unexpected errors: {:?}",
        res.errors
    );
    let json = res
        .data
        .into_json()
        .expect("data must be JSON-serialisable");
    assert_eq!(
        json["previewInviteToken"]["tokenType"],
        serde_json::json!("WORKSPACE"),
        "expected WORKSPACE token type"
    );
    assert!(
        !json["previewInviteToken"]["workspace"].is_null(),
        "workspace field must be populated for WORKSPACE token"
    );
    assert_eq!(
        json["previewInviteToken"]["workspace"]["serverName"],
        serde_json::json!("preview_ws"),
        "serverName mismatch"
    );
    assert_eq!(
        (
            &json["previewInviteToken"]["workspace"]["invitedByDisplayName"],
            &json["previewInviteToken"]["workspace"]["ownerDisplayName"],
        ),
        (
            &serde_json::json!("preview_inviter"),
            &serde_json::json!("preview_owner")
        ),
        "inviter and owner names fall back to usernames"
    );

    // ── Verify the invite row is untouched (read-only) ────────────────────
    // Re-fetch via repository to confirm use_count == 0.
    {
        use rim_auth::session_token::hash_token;
        use rim_core::auth::repository::InviteRepository;
        use rim_infra_db::sea_orm_impl::invite::SeaOrmInviteRepository;
        let invite_repo = SeaOrmInviteRepository::new(ctx.db.clone());
        let fetched = invite_repo
            .get_by_token_hash(&hash_token(raw_token))
            .await
            .expect("repo lookup failed")
            .expect("invite row must exist");
        assert_eq!(
            fetched.use_count, 0,
            "previewInviteToken must not increment use_count"
        );
    }

    // ── Unknown token → INVALID_TOKEN ─────────────────────────────────────
    let bad_query =
        r#"query { previewInviteToken(token: "completely_unknown_token_xyz") { tokenType } }"#;
    let res = schema.execute(Request::new(bad_query)).await;
    assert!(
        !res.errors.is_empty(),
        "expected INVALID_TOKEN error for unknown token"
    );
    let code = res.errors[0]
        .extensions
        .as_ref()
        .and_then(|e| e.get("code"))
        .and_then(|v| {
            if let async_graphql::Value::String(s) = v {
                Some(s.as_str())
            } else {
                None
            }
        })
        .unwrap_or("");
    assert_eq!(code, "INVALID_TOKEN", "error code mismatch: {code}");

    // ── 31st request from same IP → RATE_LIMITED ──────────────────────────
    // The rate limit is 30/5 min/IP. We drive 31 requests with the same
    // (unknown) token and the same IP injected via RemoteIp context data.
    // The first 30 return INVALID_TOKEN (unknown token), the 31st should
    // return RATE_LIMITED.
    {
        use rim_api::auth_context::RemoteIp;

        let rate_token = "rate_limit_test_token_abcdef0123456789";
        let ip = "192.0.2.99";

        for i in 0..30_u32 {
            let q =
                format!(r#"query {{ previewInviteToken(token: "{rate_token}") {{ tokenType }} }}"#);
            let req = Request::new(&q).data(RemoteIp(ip.to_string()));
            let r = schema.execute(req).await;
            let c = r
                .errors
                .first()
                .and_then(|e| e.extensions.as_ref())
                .and_then(|e| e.get("code"))
                .and_then(|v| {
                    if let async_graphql::Value::String(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("NO_ERROR");
            assert_ne!(
                c, "RATE_LIMITED",
                "request {i} should not be rate-limited yet (got {c})"
            );
        }

        // 31st request must be rate-limited.
        let q = format!(r#"query {{ previewInviteToken(token: "{rate_token}") {{ tokenType }} }}"#);
        let req = Request::new(&q).data(RemoteIp(ip.to_string()));
        let r = schema.execute(req).await;
        let code = r
            .errors
            .first()
            .and_then(|e| e.extensions.as_ref())
            .and_then(|e| e.get("code"))
            .and_then(|v| {
                if let async_graphql::Value::String(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("NO_ERROR");
        assert_eq!(
            code, "RATE_LIMITED",
            "31st request should be RATE_LIMITED (got {code})"
        );
    }
}

// ── Test 15 ──────────────────────────────────────────────────────────────────

/// Verify `signupWithInvite` end-to-end using an operator's single-use instance invite, the
/// one `rim_server invite create` issues for a fresh server's first account.
///
/// Seeds an `instance_invite` row, submits `mutation { signupWithInvite(...) }`,
/// and asserts:
/// 1. `userId` is non-null.
/// 2. `serverId` is null (the account lands in "no workspaces yet" state).
/// 3. The invite's one use is spent.
/// 4. There is no `server_member` row for the created user.
#[tokio::test]
async fn mutation_signup_with_invite_operator_invite_happy_path() {
    let ctx = TestCtx::new().await;

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    let raw_token = "test_operator_invite_e2e_abcdefghijk";
    let expires_at = Utc::now() + chrono::Duration::minutes(15);
    let invite_id = seed_operator_invite(&ctx.db, raw_token, expires_at).await;

    let mutation = format!(
        r#"mutation {{
            signupWithInvite(input: {{
                username: "newuser_operator_e2e",
                email: "newuser_operator_e2e@test.example.com",
                password: "ValidPass123!",
                instanceInviteToken: "{raw_token}"
            }}) {{
                userId
                serverId
            }}
        }}"#
    );

    let res = schema.execute(Request::new(&mutation)).await;
    assert!(
        res.errors.is_empty(),
        "signupWithInvite operator invite: unexpected errors: {:?}",
        res.errors
    );

    let json = res
        .data
        .into_json()
        .expect("data must be JSON-serialisable");
    let user_id_str = json["signupWithInvite"]["userId"]
        .as_str()
        .expect("userId must be a non-null string");
    assert!(!user_id_str.is_empty(), "userId must be non-empty");

    // serverId must be null — the new account has no workspace.
    assert!(
        json["signupWithInvite"]["serverId"].is_null(),
        "serverId must be null for an instance invite signup, got: {:?}",
        json["signupWithInvite"]["serverId"]
    );

    // ── Verify the invite's one use was spent ─────────────────────────────
    {
        use rim_core::auth::repository::InstanceInviteRepository;
        use rim_infra_db::sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository;

        let invite = SeaOrmInstanceInviteRepository::new(ctx.db.clone())
            .get_by_id(invite_id)
            .await
            .expect("repo lookup failed")
            .expect("instance_invite row must exist");
        assert_eq!(invite.use_count, 1, "the signup must spend the invite");
    }

    // ── Verify no server_member row exists for the new user ──────────────────
    {
        use sea_orm::{DbBackend, FromQueryResult, Statement};

        #[derive(Debug, FromQueryResult)]
        struct CountRow {
            cnt: i64,
        }

        let rows = CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*) AS cnt FROM server_member WHERE user_id = $1",
            [user_id_str
                .parse::<uuid::Uuid>()
                .expect("valid uuid")
                .into()],
        ))
        .all(&ctx.conn)
        .await
        .expect("server_member count query failed");

        let cnt = rows.first().map(|r| r.cnt).unwrap_or(0);
        assert_eq!(
            cnt, 0,
            "an instance invite signup must not create any server_member row"
        );
    }
}

// ── Test 40 ──────────────────────────────────────────────────────────────────

/// Verify `previewInviteToken` returns `INSTANCE` for an operator's invite and for one a user
/// issued: both are rows in the `instance_invite` table.
#[tokio::test]
async fn query_preview_invite_token_returns_instance_for_every_instance_invite() {
    let ctx = TestCtx::new().await;

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    // ── Operator invite → INSTANCE ────────────────────────────────────────
    let raw_operator_token = "test_preview_operator_type_token_abcdef";
    let expires_at = Utc::now() + chrono::Duration::minutes(30);
    let _invite_id = seed_operator_invite(&ctx.db, raw_operator_token, expires_at).await;

    let query = format!(
        r#"query {{
            previewInviteToken(token: "{raw_operator_token}") {{
                tokenType
                expiresAt
                workspace {{
                    serverId
                }}
            }}
        }}"#
    );
    let res = schema.execute(Request::new(&query)).await;
    assert!(
        res.errors.is_empty(),
        "previewInviteToken operator invite: unexpected errors: {:?}",
        res.errors
    );
    let json = res
        .data
        .into_json()
        .expect("data must be JSON-serialisable");
    assert_eq!(
        json["previewInviteToken"]["tokenType"],
        serde_json::json!("INSTANCE"),
        "an operator invite must surface as INSTANCE"
    );
    assert!(
        json["previewInviteToken"]["workspace"].is_null(),
        "workspace must be null for an instance invite"
    );
    assert!(
        !json["previewInviteToken"]["expiresAt"].is_null(),
        "expiresAt must be set for an instance invite"
    );

    // ── User-issued instance invite → INSTANCE ────────────────────────────
    use support::registration::{SeedInstanceInviteOpts, seed_instance_invite};
    let owner = ctx.create_user("preview_instance_type_owner").await;
    let raw_instance_token = "test_preview_instance_type_token_abcdef";
    let _iinv_id = seed_instance_invite(
        &ctx.db,
        owner.id,
        raw_instance_token,
        SeedInstanceInviteOpts::default(),
    )
    .await;

    let iquery = format!(
        r#"query {{
            previewInviteToken(token: "{raw_instance_token}") {{
                tokenType
            }}
        }}"#
    );
    let ires = schema.execute(Request::new(&iquery)).await;
    assert!(
        ires.errors.is_empty(),
        "previewInviteToken instance: unexpected errors: {:?}",
        ires.errors
    );
    let ijson = ires
        .data
        .into_json()
        .expect("data must be JSON-serialisable");
    assert_eq!(
        ijson["previewInviteToken"]["tokenType"],
        serde_json::json!("INSTANCE"),
        "instance invite must still surface as INSTANCE"
    );
}

/// An email-bound instance invite admits a password signup with its address only: another
/// address is a `VALIDATION` error that leaves the invite unspent, and the bound address, in
/// any case, signs up.
#[tokio::test]
async fn mutation_signup_with_an_email_bound_invite_admits_only_its_address() {
    use support::registration::{SeedInstanceInviteOpts, seed_instance_invite};

    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let owner = ctx.create_user("email_bound_invite_owner").await;
    let raw_token = "test_email_bound_invite_token_abcdef";
    seed_instance_invite(
        &ctx.db,
        owner.id,
        raw_token,
        SeedInstanceInviteOpts {
            email: Some("ops@example.com".to_string()),
            ..Default::default()
        },
    )
    .await;
    let signup = |email: &str| {
        format!(
            r#"mutation {{
                signupWithInvite(input: {{
                    username: "email_bound_user",
                    email: "{email}",
                    password: "ValidPass123!",
                    instanceInviteToken: "{raw_token}"
                }}) {{
                    userId
                }}
            }}"#
        )
    };

    let refused = schema
        .execute(Request::new(signup("someone@example.com")))
        .await;
    assert_eq!(refused.errors.len(), 1, "{:?}", refused.errors);
    let code = refused.errors[0]
        .extensions
        .as_ref()
        .and_then(|e| e.get("code"))
        .cloned();
    assert_eq!(
        code,
        Some(async_graphql::Value::String("VALIDATION".to_string()))
    );
    assert_eq!(
        refused.errors[0].message,
        "this invite admits only a password signup with the email address it was issued for"
    );

    let admitted = schema
        .execute(Request::new(signup("Ops@Example.com")))
        .await;
    assert!(admitted.errors.is_empty(), "{:?}", admitted.errors);
}

// ── Test 39 ──────────────────────────────────────────────────────────────────

/// Verify that submitting `signupWithInvite` **without** the required `email`
/// field is rejected as a GraphQL schema validation error (not a resolver error).
///
/// This proves the SDL contract: `email: String!` is enforced at the parse layer
/// before any resolver code runs.
#[tokio::test]
async fn mutation_signup_with_invite_missing_email_rejected_by_schema() {
    let ctx = TestCtx::new().await;

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    // Omit the `email` field — this violates the `email: String!` SDL contract.
    let mutation = r#"mutation {
        signupWithInvite(input: {
            username: "schema_test_user",
            password: "ValidPass123!",
            instanceInviteToken: "does_not_matter"
        }) {
            userId
        }
    }"#;

    let res = schema.execute(Request::new(mutation)).await;

    // The schema layer must reject the request with a parse/validation error
    // before the resolver is invoked. The error list must be non-empty.
    assert!(
        !res.errors.is_empty(),
        "expected schema-level error for missing required email field, but got no errors"
    );

    // At least one error must indicate the email field is missing / unknown input.
    // async-graphql surfaces this as a parse error with a message containing
    // "email" or a generic "unknown field" message.
    let error_text = res
        .errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join("; ");

    // The data must be null when there are schema validation errors.
    assert!(
        matches!(res.data, async_graphql::Value::Null),
        "data must be null when schema validation fails, got: {:?}",
        res.data
    );

    // Verify it is NOT an EMAIL_TAKEN or VALIDATION resolver error.
    let any_resolver_code = res
        .errors
        .iter()
        .any(|e| {
            e.extensions
                .as_ref()
                .and_then(|ext| ext.get("code"))
                .map(|v| matches!(v, async_graphql::Value::String(s) if s == "EMAIL_TAKEN" || s == "VALIDATION"))
                .unwrap_or(false)
        });
    assert!(
        !any_resolver_code,
        "schema error must not carry EMAIL_TAKEN or VALIDATION code (was not a resolver error): {error_text}"
    );
}

/// A `signupWithInvite` request for `username` presenting `raw` in the token field `field`.
fn signup_request(field: &str, raw: &str, username: &str, email: &str) -> Request {
    let vars = serde_json::json!({ "input": {
        "username": username, "email": email, "password": "ValidPass123!", field: raw,
    } });
    Request::new(
        "mutation Signup($input: SignupWithInviteInput!) { signupWithInvite(input: $input) { userId serverId } }",
    )
    .variables(async_graphql::Variables::from_json(vars))
}

/// Set `column` of the invite in `table` whose token is `raw` to a minute ago: `revoked_at`
/// revokes it, `expires_at` expires it.
async fn age_invite(ctx: &TestCtx, table: &str, column: &str, raw: &str) {
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    let sql =
        format!("UPDATE {table} SET {column} = now() - interval '1 minute' WHERE token_hash = $1");
    ctx.conn
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [rim_auth::session_token::hash_token(raw).into()],
        ))
        .await
        .expect("age invite");
}

/// Both kinds of invite answer an unusable token alike: unknown, revoked or used up is
/// `INVALID_TOKEN`, expired is `EXPIRED_TOKEN`, before the username or email is looked at.
#[tokio::test]
async fn signup_with_invite_answers_an_unusable_token_alike_for_both_kinds() {
    use support::graphql::{exec_err_code, exec_ok};
    use support::registration::{SeedInstanceInviteOpts, seed_instance_invite};

    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let owner = ctx.create_user("token_owner").await;
    let server = ctx.create_server("token_ws").await;
    ctx.add_owner(&server, &owner).await;

    for (field, table) in [
        ("instanceInviteToken", "instance_invite"),
        ("workspaceInviteToken", "invite"),
    ] {
        for state in ["spent", "revoked", "expired"] {
            let raw = format!("rim_{table}_{state}_token");
            if table == "invite" {
                seed_workspace_invite(&ctx.db, owner.id, server.id, &raw).await;
            } else {
                let opts = SeedInstanceInviteOpts::default();
                seed_instance_invite(&ctx.db, owner.id, &raw, opts).await;
            }
        }
        let spent = format!("rim_{table}_spent_token");
        let first = format!("{table}_first");
        let email = format!("{first}@example.com");
        exec_ok(&schema, signup_request(field, &spent, &first, &email)).await;
        age_invite(
            &ctx,
            table,
            "revoked_at",
            &format!("rim_{table}_revoked_token"),
        )
        .await;
        age_invite(
            &ctx,
            table,
            "expires_at",
            &format!("rim_{table}_expired_token"),
        )
        .await;

        for (state, code) in [
            ("unknown", "INVALID_TOKEN"),
            ("spent", "INVALID_TOKEN"),
            ("revoked", "INVALID_TOKEN"),
            ("expired", "EXPIRED_TOKEN"),
        ] {
            let raw = format!("rim_{table}_{state}_token");
            let request = signup_request(field, &raw, &first, &email);
            assert_eq!(
                exec_err_code(&schema, request).await,
                code,
                "{field} {state}"
            );
        }
    }
}

/// A workspace invite signs up with `<username>@local.invalid` and drops the email it was given,
/// however malformed: that path takes no email.
#[tokio::test]
async fn signup_with_a_workspace_invite_keeps_a_placeholder_email() {
    use rim_core::user::repository::UserRepository;
    use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
    use support::graphql::exec_ok;

    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let owner = ctx.create_user("placeholder_owner").await;
    let server = ctx.create_server("placeholder_ws").await;
    ctx.add_owner(&server, &owner).await;
    seed_workspace_invite(&ctx.db, owner.id, server.id, "rim_placeholder_token").await;

    let data = exec_ok(
        &schema,
        signup_request(
            "workspaceInviteToken",
            "rim_placeholder_token",
            "Placeholder_User",
            "not an email",
        ),
    )
    .await;
    assert_eq!(
        data["signupWithInvite"]["serverId"],
        serde_json::json!(server.id.to_string())
    );
    let user = SeaOrmUserRepository::new(ctx.db.clone())
        .find_by_username("Placeholder_User")
        .await
        .expect("lookup")
        .expect("user");
    assert_eq!(
        user.email.as_deref(),
        Some("placeholder_user@local.invalid")
    );
}

// ── The complete ban ─────────────────────────────────────────────────────────

const SIGNUP_WITH_INVITE: &str = r#"
mutation Signup($input: SignupWithInviteInput!) { signupWithInvite(input: $input) { userId } }"#;

const PASSWORD: &str = "ValidPass123!";

/// Sign `username` up with the instance invite `token`; returns the user id and session token.
async fn invited_signup(
    schema: &rim_api::AppSchema,
    username: &str,
    token: &str,
) -> (rim_core::user::model::UserId, String) {
    use std::sync::Arc;

    use rim_api::auth_intent::{AuthIntent, AuthIntentCtx};
    use serde_json::json;
    use support::graphql::{anonymous_request, exec_ok};

    let intent = Arc::new(AuthIntentCtx::default());
    let request = anonymous_request(
        SIGNUP_WITH_INVITE,
        json!({ "input": {
            "username": username,
            "email": format!("{username}@test.example.com"),
            "password": PASSWORD,
            "instanceInviteToken": token,
        } }),
    )
    .data(Arc::clone(&intent));
    let data = exec_ok(schema, request).await;
    let Some(AuthIntent::SetSession { session_token, .. }) = intent.take() else {
        panic!("signup must set a session");
    };
    let user_id = data["signupWithInvite"]["userId"]
        .as_str()
        .expect("userId")
        .parse::<uuid::Uuid>()
        .expect("uuid")
        .into();
    (user_id, session_token)
}

/// `rim_server principal …` against the test database.
async fn principal_cmd(args: &[&str]) -> String {
    let config = rim_server::config::RIMConfig {
        database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL"),
        ..Default::default()
    };
    rim_server::abuse_cmd::run_with(&config, "principal", args.iter().map(|a| a.to_string()))
        .await
        .unwrap_or_else(|e| panic!("rim_server principal {args:?}: {e}"))
}

/// Ada invited Bob, who invited Cyd. Blocking Ada with `--cascade` blocks all three: their
/// sessions end, their unused invites are revoked, their emails can neither sign up nor sign
/// in, and `principal show` lists the chain.
#[tokio::test]
async fn a_complete_ban_with_cascade_follows_the_invite_chain() {
    use rim_core::auth::repository::SessionRepository;
    use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
    use serde_json::json;
    use support::graphql::{anonymous_request, exec_err, exec_err_code};
    use support::registration::{SeedInstanceInviteOpts, seed_instance_invite};

    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let provider = ctx.build_provider();
    let ada = ctx.enrolled(&schema, "ada", 1).await;
    let principal = ada.state.principal_id.clone();

    seed_instance_invite(
        &ctx.db,
        ada.user.id,
        "rim_iinv_for_bob",
        SeedInstanceInviteOpts::default(),
    )
    .await;
    let (bob, bob_session) = invited_signup(&schema, "bob", "rim_iinv_for_bob").await;
    seed_instance_invite(
        &ctx.db,
        bob,
        "rim_iinv_for_cy",
        SeedInstanceInviteOpts {
            max_uses: Some(3),
            ..Default::default()
        },
    )
    .await;
    let (_, cy_session) = invited_signup(&schema, "cyd", "rim_iinv_for_cy").await;

    let shown = principal_cmd(&["show", &principal]).await;
    for needle in [
        "account    : ada",
        "invitees   : 2",
        "bob (no key identity)",
        "    cyd (no key identity)",
        "blocked    : no",
    ] {
        assert!(shown.contains(needle), "{needle:?} in {shown}");
    }

    let blocked = principal_cmd(&["block", &principal, "--reason", "spam ring", "--cascade"]).await;
    for needle in [
        format!("blocked ada ({principal})"),
        "blocked email ada@test.example.com".to_string(),
        "blocked bob (no key identity)".to_string(),
        "reason: spam ring (invited by ada)".to_string(),
        "blocked cyd (no key identity)".to_string(),
        "reason: spam ring (invited by bob)".to_string(),
    ] {
        assert!(blocked.contains(&needle), "{needle:?} in {blocked}");
    }

    // Every session ends: Ada's enrolling session, Bob's and Cyd's signup sessions.
    let sessions = SeaOrmSessionRepository::new(ctx.db.clone());
    let ada_session = sessions
        .get_by_id(ada.caller.session_id)
        .await
        .expect("lookup")
        .expect("session row");
    assert!(ada_session.revoked_at.is_some());
    for token in [&bob_session, &cy_session] {
        let auth = provider
            .request_authenticator()
            .authenticate_session_token(Some(token))
            .await
            .expect("authenticate");
        let request = Request::new("query { me { id } }").data(
            rim_api::auth_context::AuthActorCtx::new(auth.user_id, auth.session_id),
        );
        assert_eq!(exec_err_code(&schema, request).await, "UNAUTHENTICATED");
    }

    // Bob's password no longer signs him in, and the invite he gave Cyd, uses left, is revoked.
    let login = anonymous_request(
        "mutation($input: LoginInput!) { login(input: $input) { userId } }",
        json!({ "input": { "username": "bob", "password": PASSWORD } }),
    );
    assert_eq!(exec_err_code(&schema, login).await, "FORBIDDEN");
    let request = anonymous_request(
        SIGNUP_WITH_INVITE,
        json!({ "input": {
            "username": "dee",
            "email": "dee@test.example.com",
            "password": PASSWORD,
            "instanceInviteToken": "rim_iinv_for_cy",
        } }),
    );
    assert_eq!(exec_err_code(&schema, request).await, "INVALID_TOKEN");

    // Ada's address cannot sign up again, whatever its case.
    seed_operator_invite(
        &ctx.db,
        "rim_iinv_operator",
        Utc::now() + chrono::Duration::minutes(15),
    )
    .await;
    let request = anonymous_request(
        SIGNUP_WITH_INVITE,
        json!({ "input": {
            "username": "ada_again",
            "email": "ADA@test.example.com",
            "password": PASSWORD,
            "instanceInviteToken": "rim_iinv_operator",
        } }),
    );
    assert_eq!(
        exec_err(&schema, request).await,
        (
            "VALIDATION".to_string(),
            "this email cannot sign up here".to_string()
        )
    );

    let shown = principal_cmd(&["show", &principal]).await;
    for needle in [
        "email      : ada@test.example.com (blocked)",
        "blocked    : since",
        "bob (no key identity)",
        "  blocked",
    ] {
        assert!(shown.contains(needle), "{needle:?} in {shown}");
    }
}

/// With `RIM_RESERVED_NAMES=rim`, a signup as `rim-admin` and a profile edit to "RIM Team" are
/// refused.
#[tokio::test]
async fn a_reserved_name_is_refused_at_signup_and_on_profile_edit() {
    use serde_json::json;
    use support::graphql::{anonymous_request, exec_err};

    let ctx = TestCtx::new().await;
    let schema = ctx.build_schema_reserving("rim");
    seed_operator_invite(
        &ctx.db,
        "rim_iinv_reserved",
        Utc::now() + chrono::Duration::minutes(15),
    )
    .await;
    let refused = (
        "VALIDATION".to_string(),
        "this name is reserved".to_string(),
    );

    let signup = anonymous_request(
        SIGNUP_WITH_INVITE,
        json!({ "input": {
            "username": "rim-admin",
            "email": "rim-admin@test.example.com",
            "password": PASSWORD,
            "instanceInviteToken": "rim_iinv_reserved",
        } }),
    );
    assert_eq!(exec_err(&schema, signup).await, refused);

    let carol = ctx.create_user("carol").await;
    let caller = ctx.caller(&carol).await;
    let edit = caller.request(
        "mutation($input: UpdateMyProfileInput!) { updateMyProfile(input: $input) { id } }",
        json!({ "input": { "displayName": "RIM Team" } }),
    );
    assert_eq!(exec_err(&schema, edit).await, refused);
}
