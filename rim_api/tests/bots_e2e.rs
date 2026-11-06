//! Bot accounts through the GraphQL contract over a real database: `createBot`, `updateBot`,
//! `deactivateBot`, `createBotToken`, `revokeBotToken`, `bots`, `botTokens`, and how a bot shows
//! on `ServerMember` and `User` and stays out of everything but channels.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests --test bots_e2e -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::sync::Arc;

use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::conversation::model::ConversationId;
use rim_core::server_feature::model::ServerFeature;
use rim_core::user::model::UserId;
use rim_services::Actor;
use rim_services::auth::dto::RegistrationMode;
use rim_services::message::command::SendMessageCmd;
use rim_services::provider::AppServiceProvider;

use support::graphql::{Caller, exec_err, exec_err_code, exec_ok};
use support::roles::{Roles, create_channel};
use support::{TestCtx, unknown_id};

const BOT_FIELDS: &str = "userId serverId name runBy readsGoTo addedByName createdAt deactivatedAt";

const CREATE: &str = r#"mutation($input: CreateBotInput!) {
  createBot(input: $input) {
    token
    bot { userId serverId name runBy readsGoTo addedByName createdAt deactivatedAt }
  }
}"#;
const UPDATE: &str = r#"mutation($input: UpdateBotInput!) {
  updateBot(input: $input) { name runBy readsGoTo }
}"#;
const DEACTIVATE: &str =
    r#"mutation($id: ID!) { deactivateBot(botUserId: $id) { deactivatedAt } }"#;
const CREATE_TOKEN: &str = r#"mutation($id: ID!, $label: String!) {
  createBotToken(botUserId: $id, label: $label) { token botToken { id label revokedAt } }
}"#;
const REVOKE_TOKEN: &str =
    r#"mutation($id: ID!) { revokeBotToken(tokenId: $id) { id revokedAt } }"#;
const TOKENS: &str = r#"query($id: ID!) {
  botTokens(botUserId: $id) { id label createdAt lastUsedAt revokedAt }
}"#;
const MEMBERS: &str = r#"query($s: ID!) {
  serverMembers(serverId: $s) {
    userId memberRole isBot principalId bot { runBy readsGoTo addedByName }
  }
}"#;
const CREATE_CONVERSATION: &str = r#"mutation($input: CreateConversationInput!) {
  createConversation(input: $input) { id }
}"#;
const ADD_MEMBERS: &str = r#"mutation($s: ID!, $c: ID!, $u: [ID!]!) {
  addConversationMembers(serverId: $s, conversationId: $c, userIds: $u) { memberIds }
}"#;
const JOIN: &str = r#"mutation($s: ID!, $c: ID!) {
  joinChannel(serverId: $s, conversationId: $c) { id }
}"#;
const SET_ROLE: &str = r#"mutation($s: ID!, $u: ID!, $r: ServerMemberRoleGql!) {
  setMemberRole(serverId: $s, userId: $u, role: $r) { memberRole }
}"#;
const TRANSFER: &str = r#"mutation($s: ID!, $u: ID!) {
  transferWorkspaceOwnership(input: { serverId: $s, userId: $u }) { id }
}"#;
const MESSAGES: &str = r#"query($s: ID!, $c: ID!) {
  messages(serverId: $s, conversationId: $c, dir: AFTER, limit: 10, includeDeleted: false) {
    items { id author { id displayName isBot handle bot { runBy readsGoTo addedByName } } }
  }
}"#;

fn bots_query() -> String {
    format!("query($s: ID!) {{ bots(serverId: $s) {{ {BOT_FIELDS} }} }}")
}

struct Scene {
    ctx: TestCtx,
    roles: Roles,
    schema: AppSchema,
    provider: Arc<dyn AppServiceProvider>,
}

async fn scene(label: &str) -> Scene {
    let ctx = TestCtx::new().await;
    let roles = ctx.roles(label).await;
    let (schema, provider, _events) = ctx.build_recording(RegistrationMode::InviteOnly);
    Scene {
        ctx,
        roles,
        schema,
        provider,
    }
}

impl Scene {
    async fn ok(&self, who: &Caller, query: &str, variables: Value) -> Value {
        exec_ok(&self.schema, who.request(query, variables)).await
    }

    async fn err(&self, who: &Caller, query: &str, variables: Value) -> String {
        exec_err_code(&self.schema, who.request(query, variables)).await
    }

    /// A bot `who` creates, with its user id and first token.
    async fn create_bot(&self, who: &Caller, name: &str) -> (String, String) {
        let data = self
            .ok(
                who,
                CREATE,
                json!({ "input": {
                    "serverId": self.roles.server_id,
                    "name": name,
                    "runBy": "The docs team",
                    "readsGoTo": "A model the docs team runs",
                }}),
            )
            .await;
        (
            data["createBot"]["bot"]["userId"]
                .as_str()
                .expect("id")
                .to_string(),
            data["createBot"]["token"]
                .as_str()
                .expect("token")
                .to_string(),
        )
    }

    async fn scalar(&self, sql: &str, id: &str) -> i64 {
        self.ctx
            .conn
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                [id.parse::<uuid::Uuid>().expect("uuid").into()],
            ))
            .await
            .expect(sql)
            .expect("a row")
            .try_get::<i64>("", "n")
            .expect("n")
    }

    async fn authenticates(&self, token: &str) -> bool {
        self.provider
            .bot_service()
            .authenticate_token(token)
            .await
            .expect("answered")
            .is_some()
    }
}

fn user_id(id: &str) -> UserId {
    id.parse::<uuid::Uuid>().expect("uuid").into()
}

#[tokio::test]
async fn only_an_owner_or_moderator_creates_and_the_token_is_shown_once() {
    let s = scene("create").await;
    let r = &s.roles;
    let input = json!({ "input": {
        "serverId": r.server_id, "name": "Helper", "runBy": "Ada", "readsGoTo": "Nowhere",
    }});
    for who in [&r.bob, &r.gina, &r.olive] {
        assert_eq!(s.err(who, CREATE, input.clone()).await, "FORBIDDEN");
    }

    let (bot, token) = s.create_bot(&r.mona, "Helper").await;
    assert!(token.starts_with("rimbot_"));
    assert_eq!(token.len(), "rimbot_".len() + 43);
    let (_, second_token) = s.create_bot(&r.alice, "Helper").await;
    assert_ne!(token, second_token);

    let listed = s
        .ok(&r.bob, &bots_query(), json!({ "s": r.server_id }))
        .await;
    let found = listed["bots"]
        .as_array()
        .expect("bots")
        .iter()
        .find(|b| b["userId"] == json!(bot))
        .expect("the bot is listed");
    assert_eq!(found["name"], "Helper");
    assert_eq!(found["runBy"], "The docs team");
    assert_eq!(found["readsGoTo"], "A model the docs team runs");
    assert_eq!(found["addedByName"], "create-mona");
    assert_eq!(found["deactivatedAt"], Value::Null);
    assert_eq!(
        s.err(&r.olive, &bots_query(), json!({ "s": r.server_id }))
            .await,
        "FORBIDDEN"
    );

    let tokens = s.ok(&r.mona, TOKENS, json!({ "id": bot })).await;
    assert_eq!(tokens["botTokens"][0]["label"], "default");
    assert_eq!(tokens["botTokens"][0]["lastUsedAt"], Value::Null);
    assert_eq!(
        s.err(&r.bob, TOKENS, json!({ "id": bot })).await,
        "FORBIDDEN"
    );
    assert_eq!(
        s.err(&r.mona, TOKENS, json!({ "id": unknown_id() })).await,
        "NOT_FOUND"
    );
    assert!(!listed.to_string().contains(&token));
    assert!(!tokens.to_string().contains(&token));

    assert_eq!(
        s.scalar(
            "SELECT count(*) AS n FROM bot_token WHERE bot_user_id = $1 AND token_hash <> ''",
            &bot
        )
        .await,
        1
    );
    let raw = s
        .ctx
        .conn
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT token_hash FROM bot_token WHERE bot_user_id = $1",
            [bot.parse::<uuid::Uuid>().expect("uuid").into()],
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get::<String>("", "token_hash")
        .expect("hash");
    assert_eq!(raw, rim_auth::session_token::hash_token(&token));
    assert!(s.authenticates(&token).await);
    assert!(!s.authenticates("rimbot_unknown").await);

    let audited = s
        .scalar(
            "SELECT count(*) AS n FROM command_audit_log WHERE command_id = 'bot.create' \
             AND initiator = 'palette' AND server_id = $1",
            &r.server_id,
        )
        .await;
    assert_eq!(audited, 2);
    let leak = s
        .scalar(
            &format!(
                "SELECT count(*) AS n FROM command_audit_log WHERE server_id = $1 \
                 AND (args_redacted::text LIKE '%rimbot_%' OR args_redacted::text LIKE '%{raw}%')"
            ),
            &r.server_id,
        )
        .await;
    assert_eq!(leak, 0);
}

#[tokio::test]
async fn bad_fields_and_the_direct_space_are_refused() {
    let s = scene("bad").await;
    let r = &s.roles;
    let long = "x".repeat(201);
    for (name, run_by, reads_go_to) in [
        ("", "Ada", "Nowhere"),
        ("Helper", "", "Nowhere"),
        ("Helper", "Ada", "   "),
        ("Helper", long.as_str(), "Nowhere"),
        ("Helper", "A\nda", "Nowhere"),
    ] {
        assert_eq!(
            s.err(
                &r.alice,
                CREATE,
                json!({ "input": {
                    "serverId": r.server_id, "name": name, "runBy": run_by, "readsGoTo": reads_go_to,
                }}),
            )
            .await,
            "VALIDATION",
            "{name:?} {run_by:?} {reads_go_to:?}"
        );
    }

    let direct: rim_core::server::model::ServerId = r
        .other_server_id
        .parse::<uuid::Uuid>()
        .expect("uuid")
        .into();
    let (schema, _provider) = s.ctx.build_with_direct_space(direct);
    assert_eq!(
        exec_err_code(
            &schema,
            r.alice.request(
                CREATE,
                json!({ "input": {
                    "serverId": direct.to_string(), "name": "Helper", "runBy": "Ada",
                    "readsGoTo": "Nowhere",
                }}),
            ),
        )
        .await,
        "VALIDATION"
    );
}

#[tokio::test]
async fn a_bot_joins_as_a_guest_of_no_channel_and_shows_as_a_bot_to_members() {
    let s = scene("shown").await;
    let r = &s.roles;
    create_channel(&s.schema, &r.alice, &r.server_id, "public", false).await;

    let (bot, _) = s.create_bot(&r.mona, "Helper").await;

    assert_eq!(
        s.scalar(
            "SELECT count(*) AS n FROM conversation_member WHERE user_id = $1",
            &bot
        )
        .await,
        0,
        "joining a workspace adds a bot to no channel"
    );
    assert_eq!(
        s.err(&r.gina, &bots_query(), json!({ "s": r.server_id }))
            .await,
        "FORBIDDEN"
    );
    let members = s.ok(&r.bob, MEMBERS, json!({ "s": r.server_id })).await;
    let members = members["serverMembers"].as_array().expect("members");
    let shown = members
        .iter()
        .find(|m| m["userId"] == json!(bot))
        .expect("the bot is a member");
    assert_eq!(shown["memberRole"], "GUEST");
    assert_eq!(shown["isBot"], true);
    assert_eq!(shown["principalId"], Value::Null);
    assert_eq!(shown["bot"]["runBy"], "The docs team");
    assert_eq!(shown["bot"]["readsGoTo"], "A model the docs team runs");
    assert_eq!(shown["bot"]["addedByName"], "shown-mona");
    let person = members
        .iter()
        .find(|m| m["userId"] == json!(r.bob.user_id.to_string()))
        .expect("bob");
    assert_eq!(person["isBot"], false);
    assert_eq!(person["bot"], Value::Null);
}

#[tokio::test]
async fn a_bot_is_never_in_a_direct_message_a_group_or_an_encrypted_conversation() {
    let s = scene("dm").await;
    let r = &s.roles;
    let (bot, _) = s.create_bot(&r.mona, "Helper").await;

    for input in [
        json!({ "serverId": r.server_id, "otherUserId": bot }),
        json!({ "serverId": r.server_id, "otherUserId": bot, "encrypted": true }),
        json!({ "serverId": r.server_id, "memberIds": [bot, r.carol.user_id.to_string()] }),
        json!({ "serverId": r.server_id, "memberIds": [bot], "encrypted": true }),
    ] {
        let (code, message) = exec_err(
            &s.schema,
            r.bob
                .request(CREATE_CONVERSATION, json!({ "input": input })),
        )
        .await;
        assert_eq!(code, "VALIDATION", "{input}");
        assert_eq!(message, "A bot can only be in a channel", "{input}");
    }

    let group = s
        .ok(
            &r.bob,
            CREATE_CONVERSATION,
            json!({ "input": {
                "serverId": r.server_id, "memberIds": [r.carol.user_id.to_string()],
            }}),
        )
        .await["createConversation"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let (code, message) = exec_err(
        &s.schema,
        r.bob.request(
            ADD_MEMBERS,
            json!({ "s": r.server_id, "c": group, "u": [bot] }),
        ),
    )
    .await;
    assert_eq!(code, "VALIDATION");
    assert_eq!(message, "A bot can only be in a channel");
}

#[tokio::test]
async fn only_an_owner_or_moderator_adds_a_bot_to_a_channel_public_or_private() {
    let s = scene("add").await;
    let r = &s.roles;
    let (bot, _) = s.create_bot(&r.mona, "Helper").await;
    let public = create_channel(&s.schema, &r.mona, &r.server_id, "public", false).await;
    let private = create_channel(&s.schema, &r.mona, &r.server_id, "private", true).await;
    s.ok(&r.bob, JOIN, json!({ "s": r.server_id, "c": public }))
        .await;

    assert_eq!(
        s.err(
            &r.bob,
            ADD_MEMBERS,
            json!({ "s": r.server_id, "c": public, "u": [bot] }),
        )
        .await,
        "FORBIDDEN"
    );
    assert_eq!(
        s.scalar(
            "SELECT count(*) AS n FROM conversation_member WHERE user_id = $1",
            &bot
        )
        .await,
        0
    );

    for channel in [&public, &private] {
        let added = s
            .ok(
                &r.mona,
                ADD_MEMBERS,
                json!({ "s": r.server_id, "c": channel, "u": [bot] }),
            )
            .await;
        let ids = added["addConversationMembers"]["memberIds"]
            .as_array()
            .expect("ids");
        assert!(ids.contains(&json!(bot)), "{ids:?}");
    }

    // The bot of another workspace is no member of this one, as for anyone.
    let other = s
        .ok(
            &r.olive,
            CREATE,
            json!({ "input": {
                "serverId": r.other_server_id, "name": "Theirs", "runBy": "Olive",
                "readsGoTo": "Nowhere",
            }}),
        )
        .await["createBot"]["bot"]["userId"]
        .as_str()
        .expect("id")
        .to_string();
    assert_eq!(
        s.err(
            &r.mona,
            ADD_MEMBERS,
            json!({ "s": r.server_id, "c": public, "u": [other] }),
        )
        .await,
        "VALIDATION"
    );
}

#[tokio::test]
async fn a_moderator_adds_a_bot_to_a_public_channel_without_joining_it() {
    let s = scene("outside").await;
    let r = &s.roles;
    let (bot, _) = s.create_bot(&r.mona, "Helper").await;
    let public = create_channel(&s.schema, &r.alice, &r.server_id, "public", false).await;
    let private = create_channel(&s.schema, &r.alice, &r.server_id, "private", true).await;
    let count = "SELECT count(*) AS n FROM conversation_member WHERE user_id = $1";

    // A Member cannot, and neither can a Moderator outside a private channel.
    assert_eq!(
        s.err(
            &r.bob,
            ADD_MEMBERS,
            json!({ "s": r.server_id, "c": public, "u": [bot] }),
        )
        .await,
        "FORBIDDEN"
    );
    assert_eq!(
        s.err(
            &r.mona,
            ADD_MEMBERS,
            json!({ "s": r.server_id, "c": private, "u": [bot] }),
        )
        .await,
        "NOT_FOUND"
    );
    // A bot with a person is refused as before.
    assert_eq!(
        s.err(
            &r.mona,
            ADD_MEMBERS,
            json!({ "s": r.server_id, "c": public, "u": [bot, r.bob.user_id.to_string()] }),
        )
        .await,
        "FORBIDDEN"
    );
    assert_eq!(s.scalar(count, &bot).await, 0);

    let added = s
        .ok(
            &r.mona,
            ADD_MEMBERS,
            json!({ "s": r.server_id, "c": public, "u": [bot] }),
        )
        .await;
    assert!(
        added["addConversationMembers"]["memberIds"]
            .as_array()
            .expect("ids")
            .contains(&json!(bot))
    );
    assert_eq!(s.scalar(count, &bot).await, 1);
    let mona_id = r.mona.user_id.to_string();
    assert_eq!(s.scalar(count, &mona_id).await, 0, "mona did not join");
}

#[tokio::test]
async fn a_bot_stays_a_guest_and_owns_nothing() {
    let s = scene("guest").await;
    let r = &s.roles;
    let (bot, _) = s.create_bot(&r.mona, "Helper").await;
    for role in ["MEMBER", "MODERATOR"] {
        assert_eq!(
            s.err(
                &r.alice,
                SET_ROLE,
                json!({ "s": r.server_id, "u": bot, "r": role }),
            )
            .await,
            "VALIDATION"
        );
    }
    assert_eq!(
        s.err(&r.alice, TRANSFER, json!({ "s": r.server_id, "u": bot }))
            .await,
        "VALIDATION"
    );
}

#[tokio::test]
async fn changing_a_bot_and_managing_its_tokens() {
    let s = scene("manage").await;
    let r = &s.roles;
    let (bot, first) = s.create_bot(&r.mona, "Helper").await;

    assert_eq!(
        s.err(
            &r.bob,
            UPDATE,
            json!({ "input": { "botUserId": bot, "name": "Mine" } }),
        )
        .await,
        "FORBIDDEN"
    );
    let updated = s
        .ok(
            &r.alice,
            UPDATE,
            json!({ "input": { "botUserId": bot, "readsGoTo": "Our own server" } }),
        )
        .await;
    assert_eq!(updated["updateBot"]["name"], "Helper");
    assert_eq!(updated["updateBot"]["runBy"], "The docs team");
    assert_eq!(updated["updateBot"]["readsGoTo"], "Our own server");

    assert_eq!(
        s.err(
            &r.bob,
            CREATE_TOKEN,
            json!({ "id": bot, "label": "stolen" }),
        )
        .await,
        "FORBIDDEN"
    );
    let created = s
        .ok(&r.mona, CREATE_TOKEN, json!({ "id": bot, "label": "ci" }))
        .await;
    let second = created["createBotToken"]["token"]
        .as_str()
        .expect("token")
        .to_string();
    let token_id = created["createBotToken"]["botToken"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    assert!(s.authenticates(&second).await);

    // Five active tokens is the most: the first, this one and three more.
    for n in 0..3 {
        s.ok(
            &r.mona,
            CREATE_TOKEN,
            json!({ "id": bot, "label": format!("spare {n}") }),
        )
        .await;
    }
    assert_eq!(
        s.err(
            &r.mona,
            CREATE_TOKEN,
            json!({ "id": bot, "label": "one too many" }),
        )
        .await,
        "VALIDATION"
    );

    // Someone who moderates another workspace gets the answer of an unknown id.
    for who in [&r.olive, &r.bob] {
        assert_eq!(
            s.err(who, REVOKE_TOKEN, json!({ "id": token_id })).await,
            "NOT_FOUND"
        );
    }
    assert_eq!(
        s.err(&r.mona, REVOKE_TOKEN, json!({ "id": unknown_id() }))
            .await,
        "NOT_FOUND"
    );
    assert!(s.authenticates(&second).await);

    let revoked = s.ok(&r.mona, REVOKE_TOKEN, json!({ "id": token_id })).await;
    assert!(revoked["revokeBotToken"]["revokedAt"].is_string());
    assert!(!s.authenticates(&second).await);
    assert!(s.authenticates(&first).await);
    let again = s
        .ok(&r.alice, REVOKE_TOKEN, json!({ "id": token_id }))
        .await;
    assert_eq!(
        again["revokeBotToken"]["revokedAt"],
        revoked["revokeBotToken"]["revokedAt"]
    );

    // A revoked token no longer counts.
    s.ok(
        &r.mona,
        CREATE_TOKEN,
        json!({ "id": bot, "label": "replacement" }),
    )
    .await;

    let tokens = s.ok(&r.mona, TOKENS, json!({ "id": bot })).await;
    let dumped = tokens.to_string();
    assert!(!dumped.contains(&first) && !dumped.contains(&second));
    assert_eq!(tokens["botTokens"].as_array().expect("tokens").len(), 6);
}

#[tokio::test]
async fn deactivating_ends_the_tokens_and_the_membership_and_keeps_the_messages() {
    let s = scene("off").await;
    let r = &s.roles;
    let (bot, token) = s.create_bot(&r.mona, "Helper").await;
    let channel = create_channel(&s.schema, &r.mona, &r.server_id, "news", false).await;
    s.ok(
        &r.mona,
        ADD_MEMBERS,
        json!({ "s": r.server_id, "c": channel, "u": [bot] }),
    )
    .await;
    s.ok(&r.bob, JOIN, json!({ "s": r.server_id, "c": channel }))
        .await;
    s.provider
        .message_service()
        .send_message(SendMessageCmd {
            actor: Actor::new(user_id(&bot)),
            server_id: r.server_id.parse::<uuid::Uuid>().expect("uuid").into(),
            conversation_id: ConversationId::from(channel.parse::<uuid::Uuid>().expect("uuid")),
            text: Some("written by the bot".to_string()),
            parent_message_id: None,
            forwarded_from_message_id: None,
        })
        .await
        .expect("a bot in a channel posts through the same service");

    assert_eq!(
        s.err(&r.bob, DEACTIVATE, json!({ "id": bot })).await,
        "FORBIDDEN"
    );
    assert!(s.authenticates(&token).await);

    let done = s.ok(&r.mona, DEACTIVATE, json!({ "id": bot })).await;
    let at = done["deactivateBot"]["deactivatedAt"].clone();
    assert!(at.is_string());
    assert!(!s.authenticates(&token).await);
    assert_eq!(
        s.scalar(
            "SELECT count(*) AS n FROM server_member WHERE user_id = $1",
            &bot
        )
        .await,
        0
    );
    assert_eq!(
        s.scalar(
            "SELECT count(*) AS n FROM conversation_member WHERE user_id = $1",
            &bot
        )
        .await,
        0
    );
    assert_eq!(
        s.scalar(
            "SELECT count(*) AS n FROM bot_token WHERE bot_user_id = $1 AND revoked_at IS NULL",
            &bot
        )
        .await,
        0
    );

    let again = s.ok(&r.alice, DEACTIVATE, json!({ "id": bot })).await;
    assert_eq!(again["deactivateBot"]["deactivatedAt"], at);
    assert_eq!(
        s.scalar(
            "SELECT count(*) AS n FROM command_audit_log WHERE command_id = 'bot.deactivate' \
             AND server_id = $1",
            &r.server_id,
        )
        .await,
        1
    );

    assert_eq!(
        s.err(&r.mona, CREATE_TOKEN, json!({ "id": bot, "label": "late" }))
            .await,
        "VALIDATION"
    );

    let messages = s
        .ok(&r.bob, MESSAGES, json!({ "s": r.server_id, "c": channel }))
        .await;
    let author = messages["messages"]["items"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| &m["author"])
        .find(|a| a["id"] == json!(bot))
        .expect("the bot's message stays")
        .clone();
    assert_eq!(author["displayName"], "Helper");
    assert_eq!(author["isBot"], true);
    assert_eq!(author["handle"], Value::Null);
    assert_eq!(author["bot"]["runBy"], "The docs team");

    let listed = s
        .ok(&r.bob, &bots_query(), json!({ "s": r.server_id }))
        .await;
    assert_eq!(listed["bots"][0]["deactivatedAt"], at);
}

#[tokio::test]
async fn a_kick_or_a_ban_stops_the_token() {
    let s = scene("kick").await;
    let r = &s.roles;
    let (kicked, kicked_token) = s.create_bot(&r.mona, "Kicked").await;
    let (banned, banned_token) = s.create_bot(&r.mona, "Banned").await;
    assert!(s.authenticates(&kicked_token).await);
    s.ok(
        &r.mona,
        r#"mutation($s: ID!, $u: ID!) { kickMember(serverId: $s, userId: $u) }"#,
        json!({ "s": r.server_id, "u": kicked }),
    )
    .await;
    assert!(!s.authenticates(&kicked_token).await);

    s.ok(
        &r.mona,
        r#"mutation($s: ID!, $u: ID!) { banMember(serverId: $s, userId: $u) { expiresAt } }"#,
        json!({ "s": r.server_id, "u": banned }),
    )
    .await;
    assert!(!s.authenticates(&banned_token).await);

    s.ok(&r.mona, DEACTIVATE, json!({ "id": kicked })).await;
    assert_eq!(
        s.scalar(
            "SELECT count(*) AS n FROM bot WHERE user_id = $1 AND deactivated_at IS NOT NULL",
            &kicked
        )
        .await,
        1
    );
}

#[tokio::test]
async fn with_the_feature_off_nothing_is_managed_and_existing_bots_stay_marked() {
    let s = scene("feature").await;
    let r = &s.roles;
    let (bot, token) = s.create_bot(&r.mona, "Helper").await;
    let schema = s.ctx.build_schema_with_disabled(&[ServerFeature::Bots]);

    for (query, variables) in [
        (
            CREATE,
            json!({ "input": {
                "serverId": r.server_id, "name": "Late", "runBy": "Ada", "readsGoTo": "Nowhere",
            }}),
        ),
        (
            UPDATE,
            json!({ "input": { "botUserId": bot, "name": "Late" } }),
        ),
        (DEACTIVATE, json!({ "id": bot })),
        (CREATE_TOKEN, json!({ "id": bot, "label": "late" })),
        (REVOKE_TOKEN, json!({ "id": unknown_id() })),
        (bots_query().as_str(), json!({ "s": r.server_id })),
        (TOKENS, json!({ "id": bot })),
    ] {
        assert_eq!(
            exec_err_code(&schema, r.mona.request(query, variables)).await,
            "FEATURE_DISABLED",
            "{query}"
        );
    }

    let members = exec_ok(&schema, r.bob.request(MEMBERS, json!({ "s": r.server_id }))).await;
    let shown = members["serverMembers"]
        .as_array()
        .expect("members")
        .iter()
        .find(|m| m["userId"] == json!(bot))
        .expect("still a member")
        .clone();
    assert_eq!(shown["isBot"], true);
    assert_eq!(shown["bot"]["runBy"], "The docs team");

    let (off_provider, _) = support::build_test_provider_with_gate(
        &s.ctx.conn,
        Arc::new(rim_services::events::NoopBroadcaster),
        rim_infra_services::feature_gate::InstanceFeatureGate::new(&[ServerFeature::Bots], &[]),
    );
    assert!(
        off_provider
            .bot_service()
            .authenticate_token(&token)
            .await
            .expect("answered")
            .is_none()
    );
}

#[tokio::test]
async fn a_bots_tokens_end_with_its_workspace() {
    let s = scene("delete").await;
    let r = &s.roles;
    let (bot, token) = s.create_bot(&r.mona, "Helper").await;
    s.ok(
        &r.alice,
        r#"mutation($s: ID!, $n: String!) {
          deleteWorkspace(input: { serverId: $s, confirmName: $n }) { deleted }
        }"#,
        json!({ "s": r.server_id, "n": "delete-e2e" }),
    )
    .await;
    assert!(!s.authenticates(&token).await);
    assert_eq!(
        s.scalar(
            r#"SELECT count(*) AS n FROM "user" WHERE id = $1 AND deleted_at IS NOT NULL"#,
            &bot
        )
        .await,
        1
    );
}
