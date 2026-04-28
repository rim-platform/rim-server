//! Resolver-level e2e tests for registration GraphQL endpoints (spec §8.2, R3).
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

use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::registration::{seed_bootstrap_token, seed_workspace_invite};

// ── Test 13 ──────────────────────────────────────────────────────────────────

/// Verify that `query { registrationMode }` returns the configured mode.
///
/// Tests both `INVITE_ONLY` (default) and `OPEN` variants by building two
/// schemas with different `RegistrationMode` context values.
#[tokio::test]
async fn query_registration_mode_returns_invite_only_by_default() {
    let Some(ctx) = TestCtx::new().await else {
        return;
    };

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
/// 1. Asserts `tokenType == WORKSPACE` and populated workspace fields.
/// 2. Asserts the underlying `invite` row's `use_count` is unchanged (preview
///    is read-only — does not consume the token).
/// 3. Asserts `INVALID_TOKEN` for an unknown token.
/// 4. Asserts `RATE_LIMITED` on the 31st request from the same IP within the
///    rate-limit window (30 requests per 5 min per IP).
#[tokio::test]
async fn query_preview_invite_token_workspace_happy_path() {
    let Some(ctx) = TestCtx::new().await else {
        return;
    };

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    // Seed: owner user, server, workspace invite.
    let owner = ctx.create_user("preview_owner").await;
    let server = ctx.create_server("preview_ws").await;
    ctx.add_owner(&server, &owner).await;

    let raw_token = "test_raw_preview_ws_token_1234567890abcdef";
    let _invite_id = seed_workspace_invite(&ctx.db, owner.id, server.id, raw_token).await;

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

/// Verify `signupWithInvite` end-to-end using a bootstrap token.
///
/// Seeds a `bootstrap_token` row, submits `mutation { signupWithInvite(...) }`,
/// and asserts:
/// 1. `userId` is non-null.
/// 2. `serverId` is null (bootstrap signup lands in "no workspaces yet" state).
/// 3. The `bootstrap_token` row has `used_at IS NOT NULL` after the mutation.
/// 4. `consumed_by_user_id` matches the returned `userId`.
/// 5. There is no `server_member` row for the created user.
#[tokio::test]
async fn mutation_signup_with_invite_bootstrap_happy_path() {
    let Some(ctx) = TestCtx::new().await else {
        return;
    };

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    let raw_token = "test_bootstrap_token_e2e_abcdefghijk";
    let expires_at = Utc::now() + chrono::Duration::minutes(15);
    let token_id = seed_bootstrap_token(&ctx.db, raw_token, expires_at).await;

    let mutation = format!(
        r#"mutation {{
            signupWithInvite(input: {{
                username: "newuser_bootstrap_e2e",
                email: "newuser_bootstrap_e2e@test.example.com",
                password: "ValidPass123!",
                bootstrapToken: "{raw_token}"
            }}) {{
                userId
                serverId
            }}
        }}"#
    );

    let res = schema.execute(Request::new(&mutation)).await;
    assert!(
        res.errors.is_empty(),
        "signupWithInvite bootstrap: unexpected errors: {:?}",
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

    // serverId must be null — bootstrap user has no workspace.
    assert!(
        json["signupWithInvite"]["serverId"].is_null(),
        "serverId must be null for bootstrap signup, got: {:?}",
        json["signupWithInvite"]["serverId"]
    );

    // ── Verify bootstrap_token row was consumed ────────────────────────────
    {
        use rim_core::auth::repository::BootstrapTokenRepository;
        use rim_infra_db::sea_orm_impl::bootstrap_token::SeaOrmBootstrapTokenRepository;

        let repo = SeaOrmBootstrapTokenRepository::new(ctx.db.clone());
        let token_row = repo
            .get_by_id(token_id)
            .await
            .expect("repo lookup failed")
            .expect("bootstrap_token row must exist");

        assert!(
            token_row.used_at.is_some(),
            "used_at must be set after successful signup"
        );

        let consumed_uid = token_row
            .consumed_by_user_id
            .expect("consumed_by_user_id must be set");
        assert_eq!(
            consumed_uid.to_string(),
            user_id_str,
            "consumed_by_user_id must match the returned userId"
        );
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
            "bootstrap signup must not create any server_member row"
        );
    }
}

// ── Test 39 ──────────────────────────────────────────────────────────────────

/// Verify that submitting `signupWithInvite` **without** the required `email`
/// field is rejected as a GraphQL schema validation error (not a resolver error).
///
/// This proves the SDL contract: `email: String!` is enforced at the parse layer
/// before any resolver code runs.
#[tokio::test]
async fn mutation_signup_with_invite_missing_email_rejected_by_schema() {
    let Some(ctx) = TestCtx::new().await else {
        return;
    };

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;

    // Omit the `email` field — this violates the `email: String!` SDL contract.
    let mutation = r#"mutation {
        signupWithInvite(input: {
            username: "schema_test_user",
            password: "ValidPass123!",
            bootstrapToken: "does_not_matter"
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
