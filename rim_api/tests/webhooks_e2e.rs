//! Incoming webhooks on a real server over a real database: who may create and revoke one, what a
//! post becomes in the channel, the bytes every token that does not work gets, the gates a post
//! passes through and the limits.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests --test webhooks_e2e -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use reqwest::StatusCode;
use rim_core::server_member::model::ServerMemberRole;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};

use rim_server::config::RIMConfig;

use support::graphql::{Caller, exec_err, exec_ok};
use support::nodes::{Node, S2S_TABLES, start};
use support::roles::{Roles, create_channel};
use support::{TestCtx, unknown_id};

const CREATE_BOT: &str = r#"mutation($input: CreateBotInput!) {
  createBot(input: $input) { bot { userId } }
}"#;
const CREATE: &str = r#"mutation($input: CreateIncomingWebhookInput!) {
  createIncomingWebhook(input: $input) {
    token
    webhook { id conversationId botUserId preset createdAt lastUsedAt revokedAt }
  }
}"#;
const REVOKE: &str = "mutation($id: ID!) { revokeIncomingWebhook(id: $id) { id revokedAt } }";
const LIST: &str = r#"query($s: ID!) {
  incomingWebhooks(serverId: $s) { id conversationId botUserId preset lastUsedAt revokedAt }
}"#;
const MESSAGES: &str = r#"query($s: ID!, $c: ID!) {
  messages(serverId: $s, conversationId: $c, dir: AFTER, limit: 50, includeDeleted: false) {
    items { id text author { id isBot } }
  }
}"#;
const MEMBERS: &str = r#"query($s: ID!, $c: ID!) {
  conversation(serverId: $s, conversationId: $c) { memberIds }
}"#;
const ADD_MEMBERS: &str = r#"mutation($s: ID!, $c: ID!, $u: [ID!]!) {
  addConversationMembers(serverId: $s, conversationId: $c, userIds: $u) { memberIds }
}"#;
const REMOVE_MEMBER: &str = r#"mutation($s: ID!, $c: ID!, $u: ID!) {
  removeConversationMember(serverId: $s, conversationId: $c, userId: $u)
}"#;
const TIMEOUT: &str = r#"mutation($s: ID!, $u: ID!, $until: DateTime!) {
  timeoutMember(serverId: $s, userId: $u, until: $until) { userId }
}"#;
const BAN: &str = r#"mutation($s: ID!, $u: ID!) {
  banMember(serverId: $s, userId: $u, reason: "spam") { userId }
}"#;
const KICK: &str = "mutation($s: ID!, $u: ID!) { kickMember(serverId: $s, userId: $u) }";
const ARCHIVE: &str = r#"mutation($s: ID!, $c: ID!) {
  setConversationArchived(serverId: $s, conversationId: $c, archived: true) { id }
}"#;
const DEACTIVATE: &str = "mutation($id: ID!) { deactivateBot(botUserId: $id) { userId } }";

const BAD_TOKEN_BODY: &str = r#"{"ok":false}"#;
const POSTED_BODY: &str = r#"{"ok":true}"#;
const IGNORED_BODY: &str = r#"{"ok":true,"ignored":true}"#;

/// A workspace with its people on a running server.
struct Scene {
    ctx: TestCtx,
    roles: Roles,
    node: Node,
    http: reqwest::Client,
}

/// A webhook made by [`Scene::webhook`].
struct Hook {
    id: String,
    token: String,
}

/// What a post answered.
struct Answer {
    status: StatusCode,
    retry_after: Option<String>,
    bytes: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.bytes).expect("a JSON answer")
    }
}

async fn scene(label: &str, tweak: impl FnOnce(&mut RIMConfig)) -> Scene {
    let ctx = TestCtx::new().await;
    ctx.conn
        .execute_unprepared(S2S_TABLES)
        .await
        .expect("truncate");
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let node = start(&url, tweak).await;
    let roles = ctx.roles(label).await;
    Scene {
        ctx,
        roles,
        node,
        http: reqwest::Client::new(),
    }
}

impl Scene {
    async fn gql(&self, who: &Caller, query: &str, variables: Value) -> Value {
        exec_ok(&self.node.schema, who.request(query, variables)).await
    }

    async fn refused(&self, who: &Caller, query: &str, variables: Value) -> (String, String) {
        exec_err(&self.node.schema, who.request(query, variables)).await
    }

    async fn bot(&self, name: &str) -> String {
        self.gql(
            &self.roles.alice,
            CREATE_BOT,
            json!({ "input": {
                "serverId": self.roles.server_id,
                "name": name,
                "runBy": "The docs team",
                "readsGoTo": "Nowhere",
            }}),
        )
        .await["createBot"]["bot"]["userId"]
            .as_str()
            .expect("id")
            .to_string()
    }

    async fn channel(&self, title: &str, private: bool) -> String {
        create_channel(
            &self.node.schema,
            &self.roles.alice,
            &self.roles.server_id,
            title,
            private,
        )
        .await
    }

    fn create_vars(&self, channel: &str, bot: &str, preset: &str) -> Value {
        json!({ "input": { "conversationId": channel, "botUserId": bot, "preset": preset } })
    }

    /// A webhook of `channel` as `bot`, made by the Owner.
    async fn webhook(&self, channel: &str, bot: &str, preset: &str) -> Hook {
        let data = self
            .gql(
                &self.roles.alice,
                CREATE,
                self.create_vars(channel, bot, preset),
            )
            .await;
        let created = &data["createIncomingWebhook"];
        Hook {
            id: created["webhook"]["id"].as_str().expect("id").to_string(),
            token: created["token"].as_str().expect("token").to_string(),
        }
    }

    async fn post_to(&self, token: &str, event: Option<&str>, body: Vec<u8>) -> Answer {
        let mut request = self
            .http
            .post(format!("{}/api/hooks/{token}", self.node.origin))
            .header("content-type", "application/json")
            .body(body);
        if let Some(event) = event {
            request = request.header("x-github-event", event);
        }
        let response = request.send().await.expect("a post");
        Answer {
            status: response.status(),
            retry_after: response
                .headers()
                .get("retry-after")
                .map(|v| v.to_str().expect("retry-after").to_string()),
            bytes: response.text().await.expect("a body"),
        }
    }

    async fn post_text(&self, token: &str, text: &str) -> Answer {
        self.post_to(
            token,
            None,
            json!({ "text": text }).to_string().into_bytes(),
        )
        .await
    }

    /// The texts of `channel`'s messages by `bot`, each checked to be marked as a bot's, oldest
    /// first.
    async fn bot_texts(&self, channel: &str, bot: &str) -> Vec<String> {
        let page = self
            .gql(
                &self.roles.alice,
                MESSAGES,
                json!({ "s": self.roles.server_id, "c": channel }),
            )
            .await;
        page["messages"]["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter(|m| m["author"]["id"] == json!(bot))
            .map(|m| {
                assert_eq!(m["author"]["isBot"], true);
                m["text"].as_str().expect("text").to_string()
            })
            .collect()
    }

    /// A conversation of the workspace of the type `kind`, written directly, since starting a
    /// direct message takes the keys of both people.
    async fn conversation_row(&self, kind: &str) -> String {
        let id = uuid::Uuid::now_v7();
        self.ctx
            .conn
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO conversation (id, conversation_type, server_id) \
                 VALUES ($1, $2::text::conversation_type, $3)",
                [id.into(), kind.into(), uuid(&self.roles.server_id)],
            ))
            .await
            .expect("a conversation");
        id.to_string()
    }

    async fn count(&self, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
        self.ctx
            .conn
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .expect("count")
            .expect("a row")
            .try_get::<i64>("", "n")
            .expect("n")
    }

    async fn stop(self) {
        self.node.stop().await;
    }
}

fn uuid(id: &str) -> sea_orm::Value {
    id.parse::<uuid::Uuid>().expect("uuid").into()
}

fn release_payload() -> Value {
    json!({
        "action": "published",
        "repository": { "full_name": "acme/widgets" },
        "sender": { "login": "ada" },
        "release": {
            "name": "v1.2.0 *final*",
            "tag_name": "v1.2.0",
            "html_url": "https://github.com/acme/widgets/releases/tag/v1.2.0",
        },
    })
}

fn pull_request_payload(action: &str, merged: bool, title: &str) -> Value {
    json!({
        "action": action,
        "repository": { "full_name": "acme/widgets" },
        "sender": { "login": "ada" },
        "pull_request": {
            "number": 7,
            "title": title,
            "merged": merged,
            "html_url": "https://github.com/acme/widgets/pull/7",
        },
    })
}

fn issue_payload(action: &str) -> Value {
    json!({
        "action": action,
        "repository": { "full_name": "acme/widgets" },
        "sender": { "login": "ada" },
        "issue": {
            "number": 9,
            "title": "It broke",
            "html_url": "https://github.com/acme/widgets/issues/9",
        },
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plain_post_shows_in_the_channel_as_the_bot_and_is_audited() {
    let s = scene("plain", |_| {}).await;
    let bot = s.bot("Deploys").await;
    let ops = s.channel("ops", false).await;

    let data = s
        .gql(&s.roles.mona, CREATE, s.create_vars(&ops, &bot, "PLAIN"))
        .await;
    let created = &data["createIncomingWebhook"];
    let token = created["token"].as_str().expect("token");
    assert!(token.starts_with("rimhook_"), "{token}");
    assert_eq!(token.len(), "rimhook_".len() + 43);
    assert_eq!(created["webhook"]["conversationId"], json!(ops));
    assert_eq!(created["webhook"]["botUserId"], json!(bot));
    assert_eq!(created["webhook"]["preset"], "PLAIN");
    assert_eq!(created["webhook"]["lastUsedAt"], Value::Null);
    assert_eq!(created["webhook"]["revokedAt"], Value::Null);

    // Creating it added the bot.
    let members = s
        .gql(
            &s.roles.alice,
            MEMBERS,
            json!({ "s": s.roles.server_id, "c": ops }),
        )
        .await;
    assert!(
        members["conversation"]["memberIds"]
            .as_array()
            .expect("members")
            .contains(&json!(bot)),
        "{members}"
    );

    let answer = s.post_text(token, "  The **deploy** is done.  ").await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.bytes);
    assert_eq!(answer.bytes, POSTED_BODY);
    assert_eq!(
        s.bot_texts(&ops, &bot).await,
        ["The **deploy** is done."],
        "Markdown the sender wrote is kept"
    );

    assert_eq!(
        s.count(
            "SELECT count(*) AS n FROM command_audit_log WHERE actor_user_id = $1 \
             AND command_id = 'chat.post_message' AND initiator = 'bot'",
            vec![uuid(&bot)],
        )
        .await,
        1
    );

    // The last use shows, and no list holds a secret.
    let listed = s
        .gql(&s.roles.alice, LIST, json!({ "s": s.roles.server_id }))
        .await;
    let row = &listed["incomingWebhooks"][0];
    assert_eq!(row["conversationId"], json!(ops));
    assert!(row["lastUsedAt"].is_string(), "{listed}");
    assert!(row.get("tokenHash").is_none() && row.get("token").is_none());

    // A management entry names ids and nothing secret.
    assert_eq!(
        s.count(
            "SELECT count(*) AS n FROM command_audit_log WHERE command_id = 'bot.webhook.create' \
             AND args_redacted->>'webhook_id' = $1 AND args_redacted::text NOT LIKE '%rimhook_%'",
            vec![row["id"].as_str().expect("id").into()],
        )
        .await,
        1
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_github_webhook_posts_what_the_contract_lists_and_ignores_the_rest() {
    let s = scene("github", |_| {}).await;
    let bot = s.bot("GitHub").await;
    let ops = s.channel("ops", false).await;
    let hook = s.webhook(&ops, &bot, "GITHUB").await;

    for (event, payload) in [
        ("release", release_payload()),
        ("pull_request", pull_request_payload("opened", false, "Fix")),
        ("pull_request", pull_request_payload("closed", true, "Fix")),
        ("pull_request", pull_request_payload("closed", false, "Fix")),
        ("issues", issue_payload("opened")),
        ("issues", issue_payload("closed")),
    ] {
        let answer = s
            .post_to(&hook.token, Some(event), payload.to_string().into_bytes())
            .await;
        assert_eq!(answer.status, StatusCode::OK, "{event}: {}", answer.bytes);
        assert_eq!(answer.bytes, POSTED_BODY);
    }
    assert_eq!(
        s.bot_texts(&ops, &bot).await,
        [
            r"acme/widgets: ada published release v1.2.0 \*final\* https://github.com/acme/widgets/releases/tag/v1.2.0",
            "acme/widgets: ada opened pull request #7: Fix https://github.com/acme/widgets/pull/7",
            "acme/widgets: ada merged pull request #7: Fix https://github.com/acme/widgets/pull/7",
            "acme/widgets: ada closed pull request #7: Fix https://github.com/acme/widgets/pull/7",
            "acme/widgets: ada opened issue #9: It broke https://github.com/acme/widgets/issues/9",
            "acme/widgets: ada closed issue #9: It broke https://github.com/acme/widgets/issues/9",
        ]
    );
    let posted = s.bot_texts(&ops, &bot).await.len();

    // A ping and everything else answer 202 and post nothing, whatever the body.
    for (event, body) in [
        (
            Some("ping"),
            json!({ "zen": "Keep it simple." }).to_string(),
        ),
        (Some("ping"), "payload=%7B%7D".to_string()),
        (Some("push"), "{}".to_string()),
        (
            Some("pull_request"),
            pull_request_payload("edited", false, "Fix").to_string(),
        ),
        (Some("issues"), issue_payload("reopened").to_string()),
        (
            Some("release"),
            json!({ "action": "published" }).to_string(),
        ),
        (None, release_payload().to_string()),
    ] {
        let answer = s.post_to(&hook.token, event, body.into_bytes()).await;
        assert_eq!(answer.status, StatusCode::ACCEPTED, "{event:?}");
        assert_eq!(answer.bytes, IGNORED_BODY);
    }
    assert_eq!(s.bot_texts(&ops, &bot).await.len(), posted);

    // A handled event whose body is not JSON cannot be read.
    let answer = s
        .post_to(&hook.token, Some("issues"), b"payload=%7B%7D".to_vec())
        .await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_eq!(answer.json()["refusal"], "INVALID");
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_webhooks_text_mentions_nobody() {
    let s = scene("mentions", |_| {}).await;
    let bob = s.roles.bob.user_id.to_string();
    let token = format!("<@{bob}>");
    let bot = s.bot("Deploys").await;
    let other_bot = s.bot("Reader").await;
    let ops = s.channel("ops", false).await;
    for user in [&bob, &other_bot] {
        s.gql(
            &s.roles.alice,
            ADD_MEMBERS,
            json!({ "s": s.roles.server_id, "c": ops, "u": [user] }),
        )
        .await;
    }
    let plain = s.webhook(&ops, &bot, "PLAIN").await;
    assert_eq!(
        s.post_text(
            &plain.token,
            &format!("hello {token} <@{other_bot}> @here @CHANNEL")
        )
        .await
        .status,
        StatusCode::OK
    );
    s.gql(&s.roles.alice, REVOKE, json!({ "id": plain.id }))
        .await;
    let github = s.webhook(&ops, &bot, "GITHUB").await;
    let title = format!("Fix {token} @here");
    let answer = s
        .post_to(
            &github.token,
            Some("pull_request"),
            pull_request_payload("opened", false, &title)
                .to_string()
                .into_bytes(),
        )
        .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.bytes);

    let texts = s.bot_texts(&ops, &bot).await;
    assert_eq!(texts.len(), 2, "{texts:?}");
    for text in &texts {
        assert!(!text.contains("<@"), "{text}");
        assert!(!text.to_lowercase().contains("@here"), "{text}");
        assert!(!text.to_lowercase().contains("@channel"), "{text}");
    }
    assert_eq!(
        s.count(
            "SELECT count(*) AS n FROM message_mention m JOIN message g ON g.id = m.message_id \
             WHERE g.conversation_id = $1",
            vec![uuid(&ops)],
        )
        .await,
        0,
        "no mention row"
    );
    assert_eq!(
        s.count(
            "SELECT count(*) AS n FROM user_notification WHERE user_id = $1 AND reason = 'mention'",
            vec![uuid(&bob)],
        )
        .await,
        0,
        "no notification of a mention"
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_channel_has_one_active_webhook_and_a_revoke_makes_room() {
    let s = scene("one", |_| {}).await;
    let bot = s.bot("Deploys").await;
    let ops = s.channel("ops", false).await;
    let first = s.webhook(&ops, &bot, "PLAIN").await;

    let (code, message) = s
        .refused(&s.roles.alice, CREATE, s.create_vars(&ops, &bot, "GITHUB"))
        .await;
    assert_eq!(code, "CONFLICT");
    assert_eq!(
        message,
        "This channel already has a webhook. Revoke it first."
    );

    let revoked = s
        .gql(&s.roles.mona, REVOKE, json!({ "id": first.id }))
        .await;
    let revoked_at = revoked["revokeIncomingWebhook"]["revokedAt"].clone();
    assert!(revoked_at.is_string(), "{revoked}");
    assert_eq!(
        s.post_text(&first.token, "too late").await.bytes,
        BAD_TOKEN_BODY
    );

    // Revoking again changes nothing and writes no second entry.
    let again = s
        .gql(&s.roles.alice, REVOKE, json!({ "id": first.id }))
        .await;
    assert_eq!(again["revokeIncomingWebhook"]["revokedAt"], revoked_at);
    assert_eq!(
        s.count(
            "SELECT count(*) AS n FROM command_audit_log WHERE command_id = 'bot.webhook.revoke' \
             AND args_redacted->>'webhook_id' = $1",
            vec![first.id.as_str().into()],
        )
        .await,
        1
    );

    let second = s.webhook(&ops, &bot, "GITHUB").await;
    assert_ne!(second.token, first.token);
    let listed = s
        .gql(&s.roles.alice, LIST, json!({ "s": s.roles.server_id }))
        .await;
    assert_eq!(
        listed["incomingWebhooks"].as_array().expect("list").len(),
        2,
        "the revoked one is listed too"
    );
    assert_eq!(listed["incomingWebhooks"][0]["id"], json!(second.id));
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_moderator_of_the_workspace_manages_one() {
    let s = scene("who", |_| {}).await;
    let r = &s.roles;
    let bot = s.bot("Deploys").await;
    let ops = s.channel("ops", false).await;
    let vars = s.create_vars(&ops, &bot, "PLAIN");

    for who in [&r.bob, &r.gina] {
        let (code, _) = s.refused(who, CREATE, vars.clone()).await;
        assert_eq!(code, "FORBIDDEN");
        let (code, _) = s.refused(who, LIST, json!({ "s": r.server_id })).await;
        assert_eq!(code, "FORBIDDEN");
    }

    let hook = s.webhook(&ops, &bot, "PLAIN").await;

    // A Moderator of another workspace and an unknown id get the same answer.
    let elsewhere = s.ctx.outsider("elsewhere").await;
    s.ctx
        .join(&r.other_server_id, &elsewhere, ServerMemberRole::Moderator)
        .await;
    let (code, _) = s
        .refused(&elsewhere, REVOKE, json!({ "id": hook.id }))
        .await;
    assert_eq!(code, "NOT_FOUND");
    let (code, _) = s.refused(&r.bob, REVOKE, json!({ "id": hook.id })).await;
    assert_eq!(code, "NOT_FOUND");
    let (code, _) = s
        .refused(&r.alice, REVOKE, json!({ "id": unknown_id() }))
        .await;
    assert_eq!(code, "NOT_FOUND");
    assert_eq!(
        s.post_text(&hook.token, "still works").await.status,
        StatusCode::OK
    );

    // A bot of another workspace cannot be bound, nor one that is deactivated.
    let foreign = s
        .gql(
            &r.olive,
            CREATE_BOT,
            json!({ "input": {
                "serverId": r.other_server_id,
                "name": "Foreign",
                "runBy": "Olive",
                "readsGoTo": "Nowhere",
            }}),
        )
        .await["createBot"]["bot"]["userId"]
        .as_str()
        .expect("id")
        .to_string();
    let lobby = s.channel("lobby", false).await;
    let (code, _) = s
        .refused(&r.alice, CREATE, s.create_vars(&lobby, &foreign, "PLAIN"))
        .await;
    assert_eq!(code, "VALIDATION");
    let gone = s.bot("Gone").await;
    s.gql(&r.alice, DEACTIVATE, json!({ "id": gone })).await;
    let (code, _) = s
        .refused(&r.alice, CREATE, s.create_vars(&lobby, &gone, "PLAIN"))
        .await;
    assert_eq!(code, "VALIDATION");
    let (code, _) = s
        .refused(
            &r.alice,
            CREATE,
            s.create_vars(&unknown_id(), &bot, "PLAIN"),
        )
        .await;
    assert_eq!(code, "NOT_FOUND");
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_channel_that_is_plaintext_and_not_archived_gets_one() {
    let s = scene("kinds", |_| {}).await;
    let r = &s.roles;
    let bot = s.bot("Deploys").await;

    let dm = s.conversation_row("direct").await;
    let group = s.conversation_row("group").await;
    let archived = s.channel("attic", false).await;
    s.gql(
        &r.alice,
        ARCHIVE,
        json!({ "s": r.server_id, "c": archived }),
    )
    .await;
    for conversation in [&dm, &group, &archived] {
        let (code, _) = s
            .refused(&r.alice, CREATE, s.create_vars(conversation, &bot, "PLAIN"))
            .await;
        assert_eq!(code, "VALIDATION");
    }
    assert_eq!(
        s.count("SELECT count(*) AS n FROM incoming_webhook", vec![],)
            .await,
        0
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_private_channel_takes_a_moderator_who_is_in_it() {
    let s = scene("private", |_| {}).await;
    let r = &s.roles;
    let bot = s.bot("Deploys").await;
    let secret = s.channel("secret", true).await;
    let vars = s.create_vars(&secret, &bot, "PLAIN");

    // Mona moderates the workspace and is not in the channel.
    let (code, _) = s.refused(&r.mona, CREATE, vars.clone()).await;
    assert_eq!(code, "FORBIDDEN");

    // Nor once the bot is there: a webhook would let her post where she cannot read.
    s.gql(
        &r.alice,
        ADD_MEMBERS,
        json!({ "s": r.server_id, "c": secret, "u": [bot] }),
    )
    .await;
    let (code, _) = s.refused(&r.mona, CREATE, vars.clone()).await;
    assert_eq!(code, "FORBIDDEN");

    let hook = s.webhook(&secret, &bot, "PLAIN").await;
    assert_eq!(
        s.post_text(&hook.token, "hello").await.status,
        StatusCode::OK
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_way_a_url_stops_working_is_the_same_bytes() {
    let s = scene("stops", |_| {}).await;
    let r = &s.roles;
    let ws = &r.server_id;

    let assert_refused = async |token: &str| {
        let answer = s.post_text(token, "hello").await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{token}");
        assert_eq!(answer.bytes, BAD_TOKEN_BODY, "{token}");
    };

    assert_refused(&format!("rimhook_{}", "a".repeat(43))).await;
    assert_refused("not-a-token").await;
    assert_refused(&format!("rimbot_{}", "a".repeat(43))).await;

    let revoked_bot = s.bot("Revoked").await;
    let channel = s.channel("revoked", false).await;
    let hook = s.webhook(&channel, &revoked_bot, "PLAIN").await;
    s.gql(&r.alice, REVOKE, json!({ "id": hook.id })).await;
    assert_refused(&hook.token).await;

    let deactivated = s.bot("Deactivated").await;
    let channel = s.channel("deactivated", false).await;
    let hook = s.webhook(&channel, &deactivated, "PLAIN").await;
    s.gql(&r.alice, DEACTIVATE, json!({ "id": deactivated }))
        .await;
    assert_refused(&hook.token).await;

    // A ban and a kick take the bot out of the channel.
    let banned = s.bot("Banned").await;
    let channel = s.channel("banned", false).await;
    let hook = s.webhook(&channel, &banned, "PLAIN").await;
    s.gql(&r.alice, BAN, json!({ "s": ws, "u": banned })).await;
    assert_refused(&hook.token).await;

    let kicked = s.bot("Kicked").await;
    let channel = s.channel("kicked", false).await;
    let hook = s.webhook(&channel, &kicked, "PLAIN").await;
    s.gql(&r.alice, KICK, json!({ "s": ws, "u": kicked })).await;
    assert_refused(&hook.token).await;

    let removed = s.bot("Removed").await;
    let channel = s.channel("removed", false).await;
    let hook = s.webhook(&channel, &removed, "PLAIN").await;
    s.gql(
        &r.alice,
        REMOVE_MEMBER,
        json!({ "s": ws, "c": channel, "u": removed }),
    )
    .await;
    assert_refused(&hook.token).await;

    // A session cookie alone gets the same, and does not stand in for a token.
    let cookie = s
        .http
        .post(format!("{}/api/hooks/nope", s.node.origin))
        .header("cookie", "rim_session=abc; rim_csrf=def")
        .header("x-csrf-token", "def")
        .json(&json!({ "text": "hello" }))
        .send()
        .await
        .expect("a post");
    assert_eq!(cookie.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(cookie.text().await.expect("body"), BAD_TOKEN_BODY);

    // The feature off refuses a URL that works with it on.
    let working = s.bot("Working").await;
    let channel = s.channel("working", false).await;
    let hook = s.webhook(&channel, &working, "PLAIN").await;
    assert_eq!(s.post_text(&hook.token, "on").await.status, StatusCode::OK);
    let Scene {
        ctx,
        roles,
        node,
        http,
    } = s;
    node.stop().await;
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    ctx.conn
        .execute_unprepared(S2S_TABLES)
        .await
        .expect("truncate");
    let node = start(&url, |config| {
        config.features_disabled = Some("bots".to_string());
    })
    .await;
    let s = Scene {
        ctx,
        roles,
        node,
        http,
    };
    let answer = s.post_text(&hook.token, "off").await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(answer.bytes, BAD_TOKEN_BODY);
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_command_refuses_a_timed_out_bot_and_an_archived_channel() {
    let s = scene("gates", |_| {}).await;
    let r = &s.roles;
    let bot = s.bot("Deploys").await;
    let ops = s.channel("ops", false).await;
    let hook = s.webhook(&ops, &bot, "PLAIN").await;

    let until = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    s.gql(
        &r.alice,
        TIMEOUT,
        json!({ "s": r.server_id, "u": bot, "until": until }),
    )
    .await;
    let refused = s.post_text(&hook.token, "while timed out").await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.bytes);
    assert_eq!(refused.json()["refusal"], "DENIED");
    assert_eq!(refused.json()["retrySafe"], false);
    assert!(s.bot_texts(&ops, &bot).await.is_empty());

    let notes_bot = s.bot("Notes").await;
    let notes = s.channel("notes", false).await;
    let hook = s.webhook(&notes, &notes_bot, "PLAIN").await;
    s.gql(&r.alice, ARCHIVE, json!({ "s": r.server_id, "c": notes }))
        .await;
    let refused = s.post_text(&hook.token, "too late").await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.bytes);
    assert_eq!(refused.json()["refusal"], "DENIED");
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_thirty_first_post_is_refused_and_another_webhook_is_not_held() {
    let s = scene("limit", |config| {
        config.rate_messages_per_min_per_user = Some(1000);
        config.rate_messages_burst = Some(1000);
    })
    .await;
    let bot = s.bot("Deploys").await;
    let ops = s.channel("ops", false).await;
    let lobby = s.channel("lobby", false).await;
    let busy = s.webhook(&ops, &bot, "PLAIN").await;
    let quiet = s.webhook(&lobby, &bot, "PLAIN").await;

    for n in 1..=30 {
        let answer = s.post_text(&busy.token, &format!("post {n}")).await;
        assert_eq!(answer.status, StatusCode::OK, "{n}: {}", answer.bytes);
    }
    let limited = s.post_text(&busy.token, "one too many").await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.retry_after.is_some());
    assert_eq!(limited.json()["refusal"], "FAILED");
    assert_eq!(limited.json()["retrySafe"], true);
    assert_eq!(s.bot_texts(&ops, &bot).await.len(), 30);

    assert_eq!(
        s.post_text(&quiet.token, "still here").await.status,
        StatusCode::OK
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_past_a_limit_is_refused() {
    let s = scene("sizes", |_| {}).await;
    let bot = s.bot("Deploys").await;
    let ops = s.channel("ops", false).await;
    let hook = s.webhook(&ops, &bot, "PLAIN").await;

    let answer = s
        .post_to(&hook.token, None, vec![b' '; 1024 * 1024 + 1])
        .await;
    assert_eq!(
        answer.status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        answer.bytes
    );
    assert_eq!(answer.json()["refusal"], "INVALID");

    // A body at the limit is read, and this one is not an object with `text`.
    let answer = s.post_to(&hook.token, None, vec![b' '; 1024 * 1024]).await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.bytes);

    let answer = s.post_text(&hook.token, &"é".repeat(4_001)).await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.bytes);
    let answer = s.post_text(&hook.token, &"é".repeat(4_000)).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.bytes);

    let answer = s.post_text(&hook.token, "   ").await;
    assert_eq!(answer.status, StatusCode::ACCEPTED);
    assert_eq!(answer.bytes, IGNORED_BODY);
    assert_eq!(s.bot_texts(&ops, &bot).await.len(), 1);
    s.stop().await;
}
