//! Through-the-stack e2e tests for the workspace invite GraphQL contract: `createInvite` and
//! `joinServerByInvite`.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{
    Caller, anonymous_request, assert_each_refused, assert_each_refused_as, exec_err_code, exec_ok,
    merged,
};
use support::roles::Roles;

const CREATE_INVITE: &str = r#"
mutation Create($input: CreateInviteInput!) {
  invite: createInvite(input: $input) { inviteToken inviteId }
}
"#;

const JOIN_BY_INVITE: &str = r#"
mutation Join($input: JoinServerByInviteInput!) {
  joined: joinServerByInvite(input: $input) { serverId }
}
"#;

const SERVER_MEMBERS: &str = r#"
query Members($serverId: ID!) { serverMembers(serverId: $serverId) { userId memberRole } }
"#;

struct Fixture {
    ctx: TestCtx,
    schema: AppSchema,
    ws: Roles,
}

async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let ws = ctx.roles("invite").await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    Fixture { ctx, schema, ws }
}

fn join_vars(token: &str) -> Value {
    json!({ "input": { "inviteToken": token } })
}

impl Fixture {
    /// `createInvite` variables for the fixture workspace, with `extra` input fields.
    fn create_vars(&self, extra: Value) -> Value {
        json!({ "input": merged(json!({ "serverId": self.ws.server_id }), extra) })
    }

    /// The token of a new invite alice creates with `extra` input fields.
    async fn invite(&self, extra: Value) -> String {
        let vars = self.create_vars(extra);
        let data = exec_ok(&self.schema, self.ws.alice.request(CREATE_INVITE, vars)).await;
        data["invite"]["inviteToken"]
            .as_str()
            .expect("invite token")
            .to_string()
    }

    async fn create_code(&self, who: &Caller, extra: Value) -> String {
        let vars = self.create_vars(extra);
        exec_err_code(&self.schema, who.request(CREATE_INVITE, vars)).await
    }

    /// A signed-in user who belongs to no workspace.
    async fn newcomer(&self, name: &str) -> Caller {
        let user = self.ctx.create_user(&format!("invite-{name}")).await;
        self.ctx.caller(&user).await
    }

    async fn join(&self, who: &Caller, token: &str) -> Value {
        exec_ok(&self.schema, who.request(JOIN_BY_INVITE, join_vars(token))).await["joined"].clone()
    }

    async fn join_code(&self, who: &Caller, token: &str) -> String {
        exec_err_code(&self.schema, who.request(JOIN_BY_INVITE, join_vars(token))).await
    }

    /// The role `who` holds in the fixture workspace, or null when they are not a member.
    async fn role_of(&self, who: &Caller) -> Value {
        let vars = json!({ "serverId": self.ws.server_id });
        let data = exec_ok(&self.schema, self.ws.alice.request(SERVER_MEMBERS, vars)).await;
        data["serverMembers"]
            .as_array()
            .expect("members")
            .iter()
            .find(|m| m["userId"] == json!(who.user_id.to_string()))
            .map_or(Value::Null, |m| m["memberRole"].clone())
    }
}

/// Both invite mutations answer `UNAUTHENTICATED` without a session.
#[tokio::test]
async fn invite_mutations_refuse_anonymous_callers() {
    let f = setup().await;
    let token = f.invite(json!({})).await;
    let calls = [
        (CREATE_INVITE, f.create_vars(json!({}))),
        (JOIN_BY_INVITE, join_vars(&token)),
    ];
    assert_each_refused(&f.schema, &calls, "UNAUTHENTICATED", anonymous_request).await;
}

/// Only the owner creates invites: a moderator, a member and an outsider get `FORBIDDEN`. The
/// owner gets a `rim_inv_` token and the invite's id.
#[tokio::test]
async fn only_the_owner_creates_invites() {
    let f = setup().await;
    let calls = [(CREATE_INVITE, f.create_vars(json!({})))];
    for who in [&f.ws.mona, &f.ws.bob, &f.ws.olive] {
        assert_each_refused_as(&f.schema, who, &calls, "FORBIDDEN").await;
    }

    let vars = f.create_vars(json!({}));
    let data = exec_ok(&f.schema, f.ws.alice.request(CREATE_INVITE, vars)).await;
    let invite = &data["invite"];
    let token = invite["inviteToken"].as_str().expect("token");
    assert!(token.starts_with("rim_inv_"), "{token}");
    let invite_id = invite["inviteId"].as_str().expect("invite id");
    assert!(uuid::Uuid::parse_str(invite_id).is_ok(), "{invite_id}");
}

/// `createInvite` refuses zero uses, a moderating role, a malformed recipient address and a
/// malformed workspace id with `VALIDATION`.
#[tokio::test]
async fn create_invite_rejects_invalid_input() {
    let f = setup().await;
    for (extra, why) in [
        (json!({ "maxUses": 0 }), "zero uses"),
        (json!({ "role": "OWNER" }), "owner role"),
        (json!({ "role": "MODERATOR" }), "moderator role"),
        (
            json!({ "recipientEmail": "not-an-address" }),
            "malformed address",
        ),
        (json!({ "serverId": "nope" }), "malformed workspace id"),
    ] {
        assert_eq!(
            f.create_code(&f.ws.alice, extra).await,
            "VALIDATION",
            "{why}"
        );
    }
}

/// Joining with a valid invite answers the workspace's id and grants the invite's role: member
/// by default, guest when the invite says so.
#[tokio::test]
async fn joining_grants_the_invites_role() {
    let f = setup().await;
    let dave = f.newcomer("dave").await;
    let joined = f.join(&dave, &f.invite(json!({})).await).await;
    assert_eq!(joined, json!({ "serverId": f.ws.server_id }));
    assert_eq!(f.role_of(&dave).await, json!("MEMBER"));

    let erin = f.newcomer("erin").await;
    f.join(&erin, &f.invite(json!({ "role": "GUEST" })).await)
        .await;
    assert_eq!(f.role_of(&erin).await, json!("GUEST"));
}

/// An unknown token is `NOT_FOUND`; an expired invite and one used up to `maxUses` are
/// `FORBIDDEN`; a member presenting an invite gets `CONFLICT` without using it up. A refused
/// join adds nobody.
#[tokio::test]
async fn joining_refuses_unusable_invites() {
    let f = setup().await;
    let dave = f.newcomer("dave").await;

    assert_eq!(f.join_code(&dave, "rim_inv_unknown").await, "NOT_FOUND");
    let expired = f
        .invite(json!({ "expiresAt": chrono::Utc::now() - chrono::Duration::minutes(1) }))
        .await;
    assert_eq!(f.join_code(&dave, &expired).await, "FORBIDDEN");
    assert_eq!(f.role_of(&dave).await, Value::Null);

    let single_use = f.invite(json!({ "maxUses": 1 })).await;
    assert_eq!(f.join_code(&f.ws.bob, &single_use).await, "CONFLICT");
    assert_eq!(f.role_of(&f.ws.bob).await, json!("MEMBER"), "role kept");
    f.join(&dave, &single_use).await;
    assert_eq!(f.join_code(&f.ws.olive, &single_use).await, "FORBIDDEN");
    assert_eq!(f.role_of(&f.ws.olive).await, Value::Null);

    let two_uses = f.invite(json!({ "maxUses": 2 })).await;
    let erin = f.newcomer("erin").await;
    f.join(&erin, &two_uses).await;
    f.join(&f.ws.olive, &two_uses).await;
    let frank = f.newcomer("frank").await;
    assert_eq!(f.join_code(&frank, &two_uses).await, "FORBIDDEN");
}
