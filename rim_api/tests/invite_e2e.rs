//! Through-the-stack e2e tests for the workspace invite GraphQL contract: `createInvite`,
//! `joinServerByInvite`, `workspaceInvites` and `revokeInvite`.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use sea_orm::{ConnectionTrait, DbBackend, Statement};
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

const WORKSPACE_INVITES: &str = r#"
query Invites($serverId: ID!, $dir: PageDir, $cursor: WorkspaceInviteCursorInput, $limit: Int) {
  invites: workspaceInvites(serverId: $serverId, dir: $dir, cursor: $cursor, limit: $limit) {
    items {
      id serverId role useCount maxUses expiresAt createdAt createdById revokedAt state
      createdBy { id displayName username }
    }
    next { createdAt inviteId }
    prev { createdAt inviteId }
    pageInfo { hasNext hasPrev }
  }
}
"#;

const REVOKE_INVITE: &str = r#"
mutation Revoke($inviteId: ID!) {
  invite: revokeInvite(inviteId: $inviteId) { id state revokedAt useCount }
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

    /// The token and id of a new invite alice creates with `extra` input fields.
    async fn invite_with_id(&self, extra: Value) -> (String, String) {
        let vars = self.create_vars(extra);
        let data = exec_ok(&self.schema, self.ws.alice.request(CREATE_INVITE, vars)).await;
        let field = |name: &str| {
            data["invite"][name]
                .as_str()
                .expect("invite field")
                .to_string()
        };
        (field("inviteToken"), field("inviteId"))
    }

    /// Moves the stored expiry of the invite `invite_id` into the past.
    async fn expire(&self, invite_id: &str) {
        self.ctx
            .conn
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE invite SET expires_at = now() - interval '1 minute' WHERE id = $1",
                [uuid::Uuid::parse_str(invite_id).expect("uuid").into()],
            ))
            .await
            .expect("expire the invite");
    }

    /// The fixture workspace's invites as alice lists them, with `extra` variables.
    async fn invites(&self, extra: Value) -> Value {
        let vars = merged(json!({ "serverId": self.ws.server_id }), extra);
        exec_ok(&self.schema, self.ws.alice.request(WORKSPACE_INVITES, vars)).await["invites"]
            .clone()
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

/// The invite mutations and the invite list answer `UNAUTHENTICATED` without a session.
#[tokio::test]
async fn invite_mutations_refuse_anonymous_callers() {
    let f = setup().await;
    let (token, invite_id) = f.invite_with_id(json!({})).await;
    let calls = [
        (CREATE_INVITE, f.create_vars(json!({}))),
        (JOIN_BY_INVITE, join_vars(&token)),
        (REVOKE_INVITE, json!({ "inviteId": invite_id })),
        (WORKSPACE_INVITES, json!({ "serverId": f.ws.server_id })),
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

/// `createInvite` refuses zero uses, a moderating role, a malformed recipient address, a
/// malformed workspace id, and an expiry in the past or more than 30 days out with
/// `VALIDATION`.
#[tokio::test]
async fn create_invite_rejects_invalid_input() {
    let f = setup().await;
    let now = chrono::Utc::now();
    for (extra, why) in [
        (
            json!({ "expiresAt": now - chrono::Duration::minutes(1) }),
            "past expiry",
        ),
        (
            json!({ "expiresAt": now + chrono::Duration::days(31) }),
            "expiry over 30 days",
        ),
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
    let (expired, expired_id) = f.invite_with_id(json!({})).await;
    f.expire(&expired_id).await;
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

/// The owner creates an invite that expires in a week, lists it without its token, revokes it,
/// and a join with it is refused while the member who joined with it earlier stays.
#[tokio::test]
async fn the_owner_lists_and_revokes_an_invite() {
    let f = setup().await;
    let (token, invite_id) = f.invite_with_id(json!({ "maxUses": 3 })).await;
    let dave = f.newcomer("dave").await;
    f.join(&dave, &token).await;

    let listed = f.invites(json!({})).await;
    let items = listed["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{listed}");
    let invite = &items[0];
    assert_eq!(invite["id"], json!(invite_id));
    assert_eq!(invite["serverId"], json!(f.ws.server_id));
    assert_eq!(invite["role"], json!("MEMBER"));
    assert_eq!(invite["useCount"], json!(1));
    assert_eq!(invite["maxUses"], json!(3));
    assert_eq!(invite["state"], json!("ACTIVE"));
    assert_eq!(invite["revokedAt"], Value::Null);
    assert_eq!(invite["createdById"], json!(f.ws.alice.user_id.to_string()));
    assert_eq!(
        invite["createdBy"]["id"],
        json!(f.ws.alice.user_id.to_string())
    );
    let expires_at: chrono::DateTime<chrono::Utc> =
        serde_json::from_value(invite["expiresAt"].clone()).expect("an expiry");
    let week = chrono::Utc::now() + chrono::Duration::days(7);
    assert!((week - expires_at).num_seconds().abs() < 60, "{expires_at}");
    assert!(
        !listed.to_string().contains(&token),
        "the list must not carry the token"
    );

    let revoked = exec_ok(
        &f.schema,
        f.ws.alice
            .request(REVOKE_INVITE, json!({ "inviteId": invite_id })),
    )
    .await["invite"]
        .clone();
    assert_eq!(revoked["id"], json!(invite_id));
    assert_eq!(revoked["state"], json!("REVOKED"));
    assert!(revoked["revokedAt"].is_string(), "{revoked}");

    let again = exec_ok(
        &f.schema,
        f.ws.alice
            .request(REVOKE_INVITE, json!({ "inviteId": invite_id })),
    )
    .await["invite"]
        .clone();
    assert_eq!(again, revoked, "revoking again changes nothing");

    let erin = f.newcomer("erin").await;
    assert_eq!(f.join_code(&erin, &token).await, "FORBIDDEN");
    assert_eq!(f.role_of(&erin).await, Value::Null);
    assert_eq!(f.role_of(&dave).await, json!("MEMBER"), "dave stays");
    assert_eq!(
        f.invites(json!({})).await["items"][0]["state"],
        json!("REVOKED")
    );
}

/// Only the owner lists and revokes: a moderator, a member and a guest get `FORBIDDEN`; the
/// owner of another workspace gets `FORBIDDEN` for the list and `NOT_FOUND` for the invite,
/// as does anyone for an unknown invite. A refused revoke leaves the invite usable.
#[tokio::test]
async fn only_the_owner_lists_and_revokes_invites() {
    let f = setup().await;
    let (token, invite_id) = f.invite_with_id(json!({})).await;
    let list = (WORKSPACE_INVITES, json!({ "serverId": f.ws.server_id }));
    let revoke = (REVOKE_INVITE, json!({ "inviteId": invite_id }));
    for who in [&f.ws.mona, &f.ws.bob, &f.ws.gina] {
        assert_each_refused_as(&f.schema, who, &[list.clone(), revoke.clone()], "FORBIDDEN").await;
    }
    assert_each_refused_as(&f.schema, &f.ws.olive, &[list], "FORBIDDEN").await;
    assert_each_refused_as(&f.schema, &f.ws.olive, &[revoke], "NOT_FOUND").await;
    let unknown = (
        REVOKE_INVITE,
        json!({ "inviteId": uuid::Uuid::now_v7().to_string() }),
    );
    assert_each_refused_as(&f.schema, &f.ws.alice, &[unknown], "NOT_FOUND").await;

    let dave = f.newcomer("dave").await;
    f.join(&dave, &token).await;
    assert_eq!(f.role_of(&dave).await, json!("MEMBER"));
}

/// The list shows every invite newest first, with its state, in pages that `next` and `prev`
/// walk.
#[tokio::test]
async fn the_invite_list_pages_newest_first_with_states() {
    let f = setup().await;
    let (_, expired) = f.invite_with_id(json!({})).await;
    f.expire(&expired).await;
    let (used_up_token, used_up) = f.invite_with_id(json!({ "maxUses": 1 })).await;
    let dave = f.newcomer("dave").await;
    f.join(&dave, &used_up_token).await;
    let (_, active) = f.invite_with_id(json!({})).await;

    let first = f.invites(json!({ "limit": 2 })).await;
    let states: Vec<_> = first["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| (i["id"].clone(), i["state"].clone()))
        .collect();
    assert_eq!(
        states,
        vec![
            (json!(active), json!("ACTIVE")),
            (json!(used_up), json!("USED_UP")),
        ]
    );
    assert_eq!(
        first["pageInfo"],
        json!({ "hasNext": true, "hasPrev": false })
    );

    let second = f
        .invites(json!({ "limit": 2, "dir": "AFTER", "cursor": first["next"] }))
        .await;
    assert_eq!(second["items"][0]["id"], json!(expired));
    assert_eq!(second["items"][0]["state"], json!("EXPIRED"));
    assert_eq!(
        second["pageInfo"],
        json!({ "hasNext": false, "hasPrev": true })
    );

    let back = f
        .invites(json!({ "limit": 2, "dir": "BEFORE", "cursor": second["prev"] }))
        .await;
    assert_eq!(back["items"], first["items"]);
}
