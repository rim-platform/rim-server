//! Through-the-stack e2e tests for a workspace's public join link and its join requests:
//! `publicJoinLink`, `updatePublicJoinLink`, `reissuePublicJoinLink`, `publicJoinLinkPreview`,
//! `joinByPublicLink`, `joinRequests`, `myJoinRequests`, `acceptJoinRequest`,
//! `declineJoinRequest` and `withdrawJoinRequest`.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use async_graphql::Request;
use sea_orm::{ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Statement};
use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_api::auth_context::RemoteIp;
use rim_services::events::WorkspaceEvent;
use rim_services::limits::InstanceLimits;

use support::graphql::{Caller, anonymous_request, exec_err, exec_err_code, exec_ok};
use support::roles::Roles;
use support::{RecordingBroadcaster, TestCtx};

const LINK: &str = r#"
query Link($serverId: ID!) {
  link: publicJoinLink(serverId: $serverId) {
    serverId enabled mode role allowRemote enabledAt issuedAt issuedById token url
    joinsLastDay joinsLastWeek openRequests issuedBy { id }
  }
}
"#;

const UPDATE: &str = r#"
mutation Update($input: UpdatePublicJoinLinkInput!) {
  link: updatePublicJoinLink(input: $input) { enabled mode role allowRemote token url }
}
"#;

const REISSUE: &str = r#"
mutation Reissue($serverId: ID!) {
  link: reissuePublicJoinLink(serverId: $serverId) { enabled token url }
}
"#;

const PREVIEW: &str = r#"
query Preview($token: String!) {
  preview: publicJoinLinkPreview(token: $token) { serverId name mode viewer joinRequestId }
}
"#;

const JOIN: &str = r#"
mutation Join($token: String!, $message: String) {
  joined: joinByPublicLink(token: $token, message: $message) {
    outcome serverId joinRequest { id status message workspaceName expiresAt user { id } }
  }
}
"#;

const REQUESTS: &str = r#"
query Requests($serverId: ID!) {
  requests: joinRequests(serverId: $serverId) {
    items { id userId status message }
    pageInfo { hasNext hasPrev }
  }
}
"#;

const MY_REQUESTS: &str = r#"
query Mine { mine: myJoinRequests { id serverId workspaceName status } }
"#;

const ACCEPT: &str = r#"
mutation Accept($serverId: ID!, $requestId: ID!) {
  request: acceptJoinRequest(serverId: $serverId, requestId: $requestId) { status decidedById }
}
"#;

const DECLINE: &str = r#"
mutation Decline($serverId: ID!, $requestId: ID!) {
  request: declineJoinRequest(serverId: $serverId, requestId: $requestId) { status }
}
"#;

const WITHDRAW: &str = r#"
mutation Withdraw($requestId: ID!) {
  request: withdrawJoinRequest(requestId: $requestId) { status }
}
"#;

const BAN: &str = r#"
mutation Ban($serverId: ID!, $userId: ID!) {
  ban: banMember(serverId: $serverId, userId: $userId) { userId }
}
"#;

const SERVER_MEMBERS: &str = r#"
query Members($serverId: ID!) { serverMembers(serverId: $serverId) { userId memberRole } }
"#;

const IP: &str = "198.51.100.20";

struct Fixture {
    ctx: TestCtx,
    schema: AppSchema,
    events: std::sync::Arc<RecordingBroadcaster>,
    ws: Roles,
}

async fn setup_with(limits: InstanceLimits) -> Fixture {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles("link").await;
    let (schema, events) = ctx.build_with_limits(limits);
    Fixture {
        ctx,
        schema,
        events,
        ws,
    }
}

async fn setup() -> Fixture {
    setup_with(InstanceLimits::default()).await
}

/// `request` from the address `ip`.
fn from(request: Request, ip: &str) -> Request {
    request.data(RemoteIp::new(ip))
}

impl Fixture {
    fn server(&self) -> Value {
        json!({ "serverId": self.ws.server_id })
    }

    /// Turn the fixture workspace's link on in `mode` as its Owner; returns the token.
    async fn link_on(&self, mode: &str) -> String {
        let data = exec_ok(
            &self.schema,
            self.ws.alice.request(
                UPDATE,
                json!({ "input": { "serverId": self.ws.server_id, "enabled": true, "mode": mode } }),
            ),
        )
        .await;
        data["link"]["token"].as_str().expect("token").to_string()
    }

    async fn preview(&self, who: Option<&Caller>, token: &str, ip: &str) -> Request {
        let vars = json!({ "token": token });
        let request = match who {
            Some(who) => who.request(PREVIEW, vars),
            None => anonymous_request(PREVIEW, vars),
        };
        from(request, ip)
    }

    fn join(&self, who: &Caller, token: &str, message: Option<&str>) -> Request {
        from(
            who.request(JOIN, json!({ "token": token, "message": message })),
            IP,
        )
    }

    /// File a join request for `who` through `token`; returns its id.
    async fn ask(&self, who: &Caller, token: &str) -> String {
        let data = exec_ok(&self.schema, self.join(who, token, Some("let me in"))).await;
        assert_eq!(data["joined"]["outcome"], "REQUESTED");
        data["joined"]["joinRequest"]["id"]
            .as_str()
            .expect("request id")
            .to_string()
    }

    fn decide(&self, who: &Caller, query: &'static str, request_id: &str) -> Request {
        who.request(
            query,
            json!({ "serverId": self.ws.server_id, "requestId": request_id }),
        )
    }

    async fn role_of(&self, who: &Caller) -> Option<String> {
        let data = exec_ok(
            &self.schema,
            self.ws.alice.request(SERVER_MEMBERS, self.server()),
        )
        .await;
        data["serverMembers"]
            .as_array()
            .expect("members")
            .iter()
            .find(|m| m["userId"] == who.user_id.to_string())
            .map(|m| m["memberRole"].as_str().expect("role").to_string())
    }

    /// The audit command ids written for the fixture workspace, oldest first.
    async fn audit_ids(&self) -> Vec<(String, Value)> {
        let server: uuid::Uuid = self.ws.server_id.parse().expect("server id");
        let mut rows = entity::command_audit_log::Entity::find()
            .filter(entity::command_audit_log::Column::ServerId.eq(server))
            .all(&self.ctx.conn)
            .await
            .expect("audit rows");
        rows.sort_by_key(|r| r.id);
        rows.into_iter()
            .map(|r| (r.command_id, r.args_redacted))
            .collect()
    }

    async fn count(&self, sql: &str) -> i64 {
        let row = self
            .ctx
            .conn
            .query_one_raw(Statement::from_string(DbBackend::Postgres, sql.to_string()))
            .await
            .expect("count")
            .expect("row");
        row.try_get::<i64>("", "count").expect("count column")
    }
}

#[tokio::test]
async fn only_the_owner_reads_or_changes_the_link() {
    let f = setup().await;
    for who in [&f.ws.mona, &f.ws.bob, &f.ws.gina, &f.ws.olive] {
        let update = json!({ "input": { "serverId": f.ws.server_id, "enabled": true } });
        assert_eq!(
            exec_err_code(&f.schema, who.request(LINK, f.server())).await,
            "FORBIDDEN"
        );
        assert_eq!(
            exec_err_code(&f.schema, who.request(UPDATE, update)).await,
            "FORBIDDEN"
        );
        assert_eq!(
            exec_err_code(&f.schema, who.request(REISSUE, f.server())).await,
            "FORBIDDEN"
        );
    }
    let data = exec_ok(&f.schema, f.ws.alice.request(LINK, f.server())).await;
    assert_eq!(data["link"]["enabled"], false);
    assert_eq!(data["link"]["token"], Value::Null);
    assert_eq!(data["link"]["mode"], "DIRECT");
    assert_eq!(data["link"]["role"], "MEMBER");
    assert_eq!(data["link"]["allowRemote"], true);
}

#[tokio::test]
async fn the_owner_turns_the_link_on_and_reads_the_same_link_again() {
    let f = setup().await;
    let token = f.link_on("DIRECT").await;
    let data = exec_ok(&f.schema, f.ws.alice.request(LINK, f.server())).await;
    let link = &data["link"];
    assert_eq!(link["enabled"], true);
    assert_eq!(link["token"], token.as_str());
    assert_eq!(
        link["url"],
        format!("{}/join/{token}", support::LINK_BASE).as_str()
    );
    assert_eq!(link["issuedById"], f.ws.alice.user_id.to_string());
    assert_eq!(link["issuedBy"]["id"], f.ws.alice.user_id.to_string());
    assert!(link["enabledAt"].is_string());
    let stored = f
        .count(&format!(
            "SELECT count(*) AS count FROM workspace_public_link \
             WHERE position(convert_to('{token}', 'UTF8') IN sealed_token) > 0"
        ))
        .await;
    assert_eq!(stored, 0, "the database holds no plain token");

    let changed = exec_ok(
        &f.schema,
        f.ws.alice.request(
            UPDATE,
            json!({ "input": {
                "serverId": f.ws.server_id, "mode": "REQUESTS", "role": "GUEST",
                "allowRemote": false
            } }),
        ),
    )
    .await;
    assert_eq!(changed["link"]["mode"], "REQUESTS");
    assert_eq!(changed["link"]["role"], "GUEST");
    assert_eq!(changed["link"]["allowRemote"], false);
    assert_eq!(changed["link"]["token"], token.as_str());
    let (code, _) = exec_err(
        &f.schema,
        f.ws.alice.request(
            UPDATE,
            json!({ "input": { "serverId": f.ws.server_id, "role": "MODERATOR" } }),
        ),
    )
    .await;
    assert_eq!(code, "VALIDATION");
}

#[tokio::test]
async fn the_direct_space_and_the_default_workspace_have_no_link() {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles("nolink").await;
    let default_id: uuid::Uuid = ws.server_id.parse().expect("id");
    let schema = ctx.build_schema_with_default_workspace(
        default_id.into(),
        rim_services::auth::dto::RegistrationMode::InviteOnly,
    );
    let vars = json!({ "serverId": ws.server_id });
    assert_eq!(
        exec_err_code(&schema, ws.alice.request(LINK, vars.clone())).await,
        "VALIDATION"
    );
    assert_eq!(
        exec_err_code(&schema, ws.alice.request(REISSUE, vars)).await,
        "VALIDATION"
    );

    let direct = ctx.create_server("direct-space").await;
    let (schema, _) = ctx.build_with_direct_space(direct.id);
    assert_eq!(
        exec_err_code(
            &schema,
            ws.alice
                .request(LINK, json!({ "serverId": direct.id.to_string() }))
        )
        .await,
        "VALIDATION"
    );
}

#[tokio::test]
async fn every_invalid_link_gets_the_same_answer() {
    let f = setup().await;
    let replaced = f.link_on("DIRECT").await;
    let data = exec_ok(&f.schema, f.ws.alice.request(REISSUE, f.server())).await;
    let switched_off = data["link"]["token"].as_str().expect("token").to_string();
    exec_ok(
        &f.schema,
        f.ws.alice.request(
            UPDATE,
            json!({ "input": { "serverId": f.ws.server_id, "enabled": false } }),
        ),
    )
    .await;
    // A link of a deleted workspace.
    let gone = f.ctx.create_server("gone").await;
    let owner = f.ctx.create_user("gone-owner").await;
    f.ctx.add_owner(&gone, &owner).await;
    let gone_owner = f.ctx.caller(&owner).await;
    let data = exec_ok(
        &f.schema,
        gone_owner.request(
            UPDATE,
            json!({ "input": { "serverId": gone.id.to_string(), "enabled": true } }),
        ),
    )
    .await;
    let deleted = data["link"]["token"].as_str().expect("token").to_string();
    f.ctx
        .conn
        .execute_unprepared(&format!("DELETE FROM server WHERE id = '{}'", gone.id))
        .await
        .expect("delete workspace");

    let mut answers = Vec::new();
    for token in [
        "AAAAAAAAAAAAAAAAAAAAAA".to_string(),
        replaced,
        switched_off,
        deleted,
    ] {
        for who in [None, Some(&f.ws.olive)] {
            answers.push(exec_err(&f.schema, f.preview(who, &token, IP).await).await);
        }
        answers.push(exec_err(&f.schema, f.join(&f.ws.olive, &token, None)).await);
    }
    for answer in &answers {
        assert_eq!(answer, &("NOT_FOUND".to_string(), "not found".to_string()));
    }
}

#[tokio::test]
async fn the_preview_needs_no_sign_in_and_tells_a_caller_where_they_stand() {
    let f = setup().await;
    let token = f.link_on("REQUESTS").await;
    let data = exec_ok(&f.schema, f.preview(None, &token, IP).await).await;
    assert_eq!(
        data["preview"],
        json!({
            "serverId": f.ws.server_id, "name": "link-e2e", "mode": "REQUESTS",
            "viewer": "SIGNED_OUT", "joinRequestId": null
        })
    );
    let viewer = |data: &Value| data["preview"]["viewer"].as_str().unwrap().to_string();
    let data = exec_ok(&f.schema, f.preview(Some(&f.ws.bob), &token, IP).await).await;
    assert_eq!(viewer(&data), "MEMBER");
    let data = exec_ok(&f.schema, f.preview(Some(&f.ws.olive), &token, IP).await).await;
    assert_eq!(viewer(&data), "NOT_MEMBER");
    let request_id = f.ask(&f.ws.olive, &token).await;
    let data = exec_ok(&f.schema, f.preview(Some(&f.ws.olive), &token, IP).await).await;
    assert_eq!(viewer(&data), "REQUEST_OPEN");
    assert_eq!(data["preview"]["joinRequestId"], request_id.as_str());
    let banned = f.ctx.outsider("link-banned").await;
    exec_ok(
        &f.schema,
        f.ws.mona.request(
            BAN,
            json!({ "serverId": f.ws.server_id, "userId": banned.user_id.to_string() }),
        ),
    )
    .await;
    let data = exec_ok(&f.schema, f.preview(Some(&banned), &token, IP).await).await;
    assert_eq!(viewer(&data), "BANNED");
}

#[tokio::test]
async fn twenty_misses_cut_an_address_off_and_successes_do_not_count() {
    let f = setup().await;
    let token = f.link_on("DIRECT").await;
    for _ in 0..25 {
        exec_ok(&f.schema, f.preview(None, &token, IP).await).await;
    }
    for i in 0..10 {
        let miss = f.preview(None, &format!("wrong-{i}"), IP).await;
        assert_eq!(exec_err_code(&f.schema, miss).await, "NOT_FOUND");
        let miss = from(
            f.ws.olive
                .request(JOIN, json!({ "token": format!("bad-{i}") })),
            IP,
        );
        assert_eq!(exec_err_code(&f.schema, miss).await, "NOT_FOUND");
    }
    let res = f.schema.execute(f.preview(None, &token, IP).await).await;
    let ext = res.errors[0].extensions.as_ref().expect("extensions");
    assert_eq!(
        ext.get("code"),
        Some(&async_graphql::Value::from("RATE_LIMITED"))
    );
    let retry_after = match ext.get("retryAfterSeconds") {
        Some(async_graphql::Value::Number(n)) => n.as_i64().expect("seconds"),
        other => panic!("no retryAfterSeconds: {other:?}"),
    };
    assert!((3590..=3600).contains(&retry_after), "{retry_after}");
    assert_eq!(
        exec_err_code(&f.schema, f.join(&f.ws.olive, &token, None)).await,
        "RATE_LIMITED"
    );
    exec_ok(&f.schema, f.preview(None, &token, "203.0.113.50").await).await;
}

#[tokio::test]
async fn a_direct_join_adds_the_member_with_the_links_role_and_public_channels() {
    let f = setup().await;
    let channel =
        support::roles::create_channel(&f.schema, &f.ws.alice, &f.ws.server_id, "general", false)
            .await;
    let token = f.link_on("DIRECT").await;
    let data = exec_ok(&f.schema, f.join(&f.ws.olive, &token, None)).await;
    assert_eq!(
        data["joined"],
        json!({ "outcome": "JOINED", "serverId": f.ws.server_id, "joinRequest": null })
    );
    assert_eq!(f.role_of(&f.ws.olive).await.as_deref(), Some("MEMBER"));
    let joined_channel = f
        .count(&format!(
            "SELECT count(*) AS count FROM conversation_member \
             WHERE conversation_id = '{channel}' AND user_id = '{}'",
            f.ws.olive.user_id
        ))
        .await;
    assert_eq!(joined_channel, 1);
    assert!(f.events.events().iter().any(|e| matches!(
        e,
        WorkspaceEvent::MemberJoined { user_id, source: Some(source), .. }
            if *user_id == f.ws.olive.user_id && source == "public_link"
    )));

    exec_ok(
        &f.schema,
        f.ws.alice.request(
            UPDATE,
            json!({ "input": { "serverId": f.ws.server_id, "role": "GUEST" } }),
        ),
    )
    .await;
    let guest = f.ctx.outsider("link-guest").await;
    exec_ok(&f.schema, f.join(&guest, &token, None)).await;
    assert_eq!(f.role_of(&guest).await.as_deref(), Some("GUEST"));

    let data = exec_ok(&f.schema, f.ws.alice.request(LINK, f.server())).await;
    assert_eq!(data["link"]["joinsLastDay"], 2);
    assert_eq!(data["link"]["joinsLastWeek"], 2);

    let audit = f.audit_ids().await;
    let ids: Vec<&str> = audit.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "public_link.enabled",
            "workspace.member_joined",
            "public_link.settings_changed",
            "workspace.member_joined",
        ]
    );
    for (_, args) in &audit {
        assert!(!args.to_string().contains(&token), "no token in the audit");
    }
    assert_eq!(audit[1].1["source"], "public_link");
}

#[tokio::test]
async fn a_new_token_ends_the_old_one_at_once() {
    let f = setup().await;
    let old = f.link_on("DIRECT").await;
    let data = exec_ok(&f.schema, f.ws.alice.request(REISSUE, f.server())).await;
    let new = data["link"]["token"].as_str().expect("token").to_string();
    assert_ne!(old, new);
    assert_eq!(
        exec_err_code(&f.schema, f.join(&f.ws.olive, &old, None)).await,
        "NOT_FOUND"
    );
    exec_ok(&f.schema, f.join(&f.ws.olive, &new, None)).await;
    let ids: Vec<String> = f.audit_ids().await.into_iter().map(|(id, _)| id).collect();
    assert!(ids.contains(&"public_link.reissued".to_string()));
}

#[tokio::test]
async fn each_check_refuses_a_join() {
    // Five members: alice, mona, bob, carol and gina.
    let f = setup_with(InstanceLimits {
        members_per_workspace: Some(6),
        workspaces_per_user: Some(1),
        ..Default::default()
    })
    .await;
    let token = f.link_on("DIRECT").await;

    assert_eq!(
        exec_err_code(&f.schema, f.join(&f.ws.bob, &token, None)).await,
        "CONFLICT"
    );
    let banned = f.ctx.outsider("link-banned").await;
    exec_ok(
        &f.schema,
        f.ws.mona.request(
            BAN,
            json!({ "serverId": f.ws.server_id, "userId": banned.user_id.to_string() }),
        ),
    )
    .await;
    assert_eq!(
        exec_err(&f.schema, f.join(&banned, &token, None)).await,
        (
            "FORBIDDEN".to_string(),
            "you are banned from this workspace".to_string()
        )
    );
    // olive owns a workspace already, at the cap of one owned workspace: joining does not make
    // anyone an owner, so the cap does not refuse it.
    exec_ok(&f.schema, f.join(&f.ws.olive, &token, None)).await;
    let late = f.ctx.outsider("link-late").await;
    assert_eq!(
        exec_err(&f.schema, f.join(&late, &token, None)).await,
        (
            "VALIDATION".to_string(),
            "this instance allows 6 members per workspace".to_string()
        )
    );
    assert_eq!(f.role_of(&late).await, None);
    assert_eq!(f.role_of(&banned).await, None);
}

#[tokio::test]
async fn the_link_never_creates_an_account() {
    let f = setup().await;
    let token = f.link_on("DIRECT").await;
    let users_before = f.count(r#"SELECT count(*) AS count FROM "user""#).await;

    let signup = format!(
        r#"mutation {{
            signupWithInvite(input: {{
                username: "linkjoiner", email: "linkjoiner@test.example.com",
                password: "ValidPass123!", workspaceInviteToken: "{token}"
            }}) {{ userId }}
        }}"#
    );
    let intent = std::sync::Arc::new(rim_api::auth_intent::AuthIntentCtx::default());
    let res = f
        .schema
        .execute(from(Request::new(&signup).data(intent), IP))
        .await;
    let code = res.errors[0]
        .extensions
        .as_ref()
        .and_then(|e| e.get("code"));
    assert_eq!(code, Some(&async_graphql::Value::from("INVALID_TOKEN")));

    let preview = format!(r#"query {{ previewInviteToken(token: "{token}") {{ tokenType }} }}"#);
    let res = f.schema.execute(from(Request::new(&preview), IP)).await;
    assert!(!res.errors.is_empty(), "not an invite");

    let anonymous = from(anonymous_request(JOIN, json!({ "token": token })), IP);
    assert_eq!(exec_err_code(&f.schema, anonymous).await, "UNAUTHENTICATED");
    assert_eq!(
        f.count(r#"SELECT count(*) AS count FROM "user""#).await,
        users_before
    );
}

#[tokio::test]
async fn join_requests_are_filed_listed_and_accepted() {
    let f = setup().await;
    let token = f.link_on("REQUESTS").await;
    let data = exec_ok(&f.schema, f.join(&f.ws.olive, &token, Some("  hi  "))).await;
    let joined = &data["joined"];
    assert_eq!(joined["outcome"], "REQUESTED");
    assert_eq!(joined["joinRequest"]["status"], "OPEN");
    assert_eq!(joined["joinRequest"]["message"], "hi");
    assert_eq!(joined["joinRequest"]["workspaceName"], "link-e2e");
    let request_id = joined["joinRequest"]["id"].as_str().unwrap().to_string();
    assert_eq!(f.role_of(&f.ws.olive).await, None);
    assert_eq!(
        exec_err_code(&f.schema, f.join(&f.ws.olive, &token, None)).await,
        "CONFLICT"
    );
    let long = "x".repeat(501);
    let other = f.ctx.outsider("link-long").await;
    assert_eq!(
        exec_err_code(&f.schema, f.join(&other, &token, Some(&long))).await,
        "VALIDATION"
    );

    let data = exec_ok(&f.schema, f.ws.olive.request(MY_REQUESTS, json!({}))).await;
    assert_eq!(data["mine"][0]["id"], request_id.as_str());
    for who in [&f.ws.alice, &f.ws.mona] {
        let data = exec_ok(&f.schema, who.request(REQUESTS, f.server())).await;
        assert_eq!(data["requests"]["items"][0]["id"], request_id.as_str());
    }
    for who in [&f.ws.bob, &f.ws.gina, &f.ws.olive] {
        assert_eq!(
            exec_err_code(&f.schema, who.request(REQUESTS, f.server())).await,
            "FORBIDDEN"
        );
        assert_eq!(
            exec_err_code(&f.schema, f.decide(who, ACCEPT, &request_id)).await,
            "FORBIDDEN"
        );
    }
    let data = exec_ok(&f.schema, f.ws.alice.request(LINK, f.server())).await;
    assert_eq!(data["link"]["openRequests"], 1);

    let created = f.events.events().into_iter().find_map(|e| match e {
        WorkspaceEvent::JoinRequestCreated {
            member_user_ids, ..
        } => Some(member_user_ids),
        _ => None,
    });
    let audience = created.expect("announced");
    assert!(audience.contains(&f.ws.alice.user_id));
    assert!(audience.contains(&f.ws.mona.user_id));
    assert!(audience.contains(&f.ws.olive.user_id));
    assert!(!audience.contains(&f.ws.bob.user_id));

    let data = exec_ok(&f.schema, f.decide(&f.ws.mona, ACCEPT, &request_id)).await;
    assert_eq!(data["request"]["status"], "ACCEPTED");
    assert_eq!(
        data["request"]["decidedById"],
        f.ws.mona.user_id.to_string()
    );
    assert_eq!(f.role_of(&f.ws.olive).await.as_deref(), Some("MEMBER"));
    assert_eq!(
        exec_err_code(&f.schema, f.decide(&f.ws.alice, ACCEPT, &request_id)).await,
        "CONFLICT"
    );
    assert!(f.events.events().iter().any(|e| matches!(
        e,
        WorkspaceEvent::JoinRequestDecided { user_id, outcome, .. }
            if *user_id == f.ws.olive.user_id && outcome == "accepted"
    )));
    let ids: Vec<String> = f.audit_ids().await.into_iter().map(|(id, _)| id).collect();
    assert!(ids.contains(&"join_request.accepted".to_string()));
    assert!(ids.contains(&"workspace.member_joined".to_string()));
    let data = exec_ok(&f.schema, f.ws.alice.request(LINK, f.server())).await;
    assert_eq!(data["link"]["openRequests"], 0);
    assert_eq!(data["link"]["joinsLastDay"], 1);
}

#[tokio::test]
async fn a_declined_person_waits_a_day_and_a_withdrawn_one_may_ask_again() {
    let f = setup().await;
    let token = f.link_on("REQUESTS").await;
    let request_id = f.ask(&f.ws.olive, &token).await;
    let data = exec_ok(&f.schema, f.decide(&f.ws.alice, DECLINE, &request_id)).await;
    assert_eq!(data["request"]["status"], "DECLINED");
    let (code, message) = exec_err(&f.schema, f.join(&f.ws.olive, &token, None)).await;
    assert_eq!(code, "VALIDATION");
    assert!(message.contains("again in 24 hours"), "{message}");
    f.ctx
        .conn
        .execute_unprepared(
            "UPDATE workspace_join_request SET decided_at = now() - interval '25 hours'",
        )
        .await
        .expect("age the decline");
    let again = f.ask(&f.ws.olive, &token).await;

    assert_eq!(
        exec_err_code(
            &f.schema,
            f.ws.bob.request(WITHDRAW, json!({ "requestId": again }))
        )
        .await,
        "NOT_FOUND"
    );
    let data = exec_ok(
        &f.schema,
        f.ws.olive.request(WITHDRAW, json!({ "requestId": again })),
    )
    .await;
    assert_eq!(data["request"]["status"], "WITHDRAWN");
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.ws.olive.request(WITHDRAW, json!({ "requestId": again }))
        )
        .await,
        "CONFLICT"
    );
    f.ask(&f.ws.olive, &token).await;
    let ids: Vec<String> = f.audit_ids().await.into_iter().map(|(id, _)| id).collect();
    assert!(ids.contains(&"join_request.declined".to_string()));
}

#[tokio::test]
async fn a_person_holds_at_most_twenty_open_requests_and_requests_expire() {
    let f = setup().await;
    let token = f.link_on("REQUESTS").await;
    for i in 0..20 {
        let server = f.ctx.create_server(&format!("asked-{i}")).await;
        f.ctx
            .conn
            .execute_unprepared(&format!(
                "INSERT INTO workspace_join_request (id, server_id, user_id, expires_at) \
                 VALUES (gen_random_uuid(), '{}', '{}', now() + interval '30 days')",
                server.id, f.ws.olive.user_id
            ))
            .await
            .expect("seed request");
    }
    let (code, message) = exec_err(&f.schema, f.join(&f.ws.olive, &token, None)).await;
    assert_eq!(code, "VALIDATION");
    assert!(
        message.contains("at most 20 open join requests"),
        "{message}"
    );

    // Every request reaches its expiry: none is open any more.
    f.ctx
        .conn
        .execute_unprepared(
            "UPDATE workspace_join_request SET created_at = now() - interval '31 days', \
             expires_at = now() - interval '1 day'",
        )
        .await
        .expect("expire");
    let data = exec_ok(&f.schema, f.ws.olive.request(MY_REQUESTS, json!({}))).await;
    assert_eq!(data["mine"], json!([]));
    let request_id = f.ask(&f.ws.olive, &token).await;
    f.ctx
        .conn
        .execute_unprepared(&format!(
            "UPDATE workspace_join_request SET expires_at = now() - interval '1 second' \
             WHERE id = '{request_id}'"
        ))
        .await
        .expect("expire");
    let data = exec_ok(&f.schema, f.ws.alice.request(REQUESTS, f.server())).await;
    assert_eq!(data["requests"]["items"], json!([]));
    assert_eq!(
        exec_err_code(&f.schema, f.decide(&f.ws.alice, ACCEPT, &request_id)).await,
        "CONFLICT"
    );
}

#[tokio::test]
async fn a_ban_cancels_open_requests_and_refuses_new_ones() {
    let f = setup().await;
    let token = f.link_on("REQUESTS").await;
    let request_id = f.ask(&f.ws.olive, &token).await;
    exec_ok(
        &f.schema,
        f.ws.mona.request(
            BAN,
            json!({ "serverId": f.ws.server_id, "userId": f.ws.olive.user_id.to_string() }),
        ),
    )
    .await;
    let data = exec_ok(&f.schema, f.ws.olive.request(MY_REQUESTS, json!({}))).await;
    assert_eq!(data["mine"], json!([]));
    assert_eq!(
        exec_err_code(&f.schema, f.decide(&f.ws.alice, ACCEPT, &request_id)).await,
        "CONFLICT"
    );
    assert_eq!(
        exec_err_code(&f.schema, f.join(&f.ws.olive, &token, None)).await,
        "FORBIDDEN"
    );
    assert!(f.events.events().iter().any(|e| matches!(
        e,
        WorkspaceEvent::JoinRequestDecided { outcome, .. } if outcome == "cancelled"
    )));
}
