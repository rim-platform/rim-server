//! The bot API on a real server over a real database: every call, the bytes a bot sees for what
//! it cannot reach, the gates a command applies to a bot, and polling for mentions and events.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests --test bot_api_e2e -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use reqwest::{Method, StatusCode};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};

use rim_core::server_member::model::ServerMemberRole;
use rim_server::config::RIMConfig;

use support::graphql::{Caller, exec_err_code, exec_ok};
use support::nodes::{Node, S2S_TABLES, start};
use support::relay::{RESET, key_signup, keys, open_registration};
use support::roles::{Roles, create_channel};
use support::{TestCtx, unknown_id};

const CREATE_BOT: &str = r#"mutation($input: CreateBotInput!) {
  createBot(input: $input) { token bot { userId } }
}"#;
const ADD_MEMBERS: &str = r#"mutation($s: ID!, $c: ID!, $u: [ID!]!) {
  addConversationMembers(serverId: $s, conversationId: $c, userIds: $u) { memberIds }
}"#;
const REMOVE_MEMBER: &str = r#"mutation($s: ID!, $c: ID!, $u: ID!) {
  removeConversationMember(serverId: $s, conversationId: $c, userId: $u)
}"#;
const SEND: &str = r#"mutation($input: SendMessageInput!) { sendMessage(input: $input) { id } }"#;
const DELETE: &str = r#"mutation($input: DeleteMessageInput!) { deleteMessage(input: $input) }"#;
const REACT: &str =
    r#"mutation($input: ToggleReactionInput!) { toggleReaction(input: $input) { emoji } }"#;
const MESSAGES: &str = r#"query($s: ID!, $c: ID!) {
  messages(serverId: $s, conversationId: $c, dir: AFTER, limit: 50, includeDeleted: false) {
    items { id text author { id displayName isBot } }
  }
}"#;
const TIMEOUT: &str = r#"mutation($s: ID!, $u: ID!, $until: DateTime!) {
  timeoutMember(serverId: $s, userId: $u, until: $until) { userId }
}"#;
const CLEAR_TIMEOUT: &str = "mutation($s: ID!, $u: ID!) { clearTimeout(serverId: $s, userId: $u) }";
const BAN: &str = r#"mutation($s: ID!, $u: ID!) {
  banMember(serverId: $s, userId: $u, reason: "spam") { userId }
}"#;
const KICK: &str = "mutation($s: ID!, $u: ID!) { kickMember(serverId: $s, userId: $u) }";
const SLOW_MODE: &str = r#"mutation($s: ID!, $c: ID!, $seconds: Int!) {
  setSlowMode(serverId: $s, conversationId: $c, seconds: $seconds) { id }
}"#;
const POSTING: &str = r#"mutation($s: ID!, $c: ID!) {
  setChannelPosting(serverId: $s, conversationId: $c, policy: MODERATORS, allowsThreads: false) { id }
}"#;
const ARCHIVE: &str = r#"mutation($s: ID!, $c: ID!) {
  setConversationArchived(serverId: $s, conversationId: $c, archived: true) { id }
}"#;
const REPLY: &str =
    r#"mutation($input: ReplyToMessageInput!) { replyToMessage(input: $input) { id } }"#;
const UPDATE_BOT: &str = r#"mutation($input: UpdateBotInput!) {
  updateBot(input: $input) { userId seesMemberJoins }
}"#;
const LIST_BOTS: &str = "query($s: ID!) { bots(serverId: $s) { userId seesMemberJoins } }";
const DEACTIVATE: &str = "mutation($id: ID!) { deactivateBot(botUserId: $id) { userId } }";
const TOKENS: &str = "query($id: ID!) { botTokens(botUserId: $id) { id } }";
const REVOKE: &str = "mutation($id: ID!) { revokeBotToken(tokenId: $id) { id } }";

const NOT_FOUND_BODY: &str =
    r#"{"refusal":"INVALID","message":"There is nothing here.","retrySafe":false}"#;
const BAD_TOKEN_BODY: &str =
    r#"{"refusal":"DENIED","message":"This token isn't valid.","retrySafe":false}"#;

/// A workspace with its people on a running server.
struct Scene {
    ctx: TestCtx,
    roles: Roles,
    node: Node,
    http: reqwest::Client,
}

/// A bot of the workspace, with its first token.
struct TestBot {
    id: String,
    token: String,
}

/// What a call answered.
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

    async fn bot(&self, name: &str) -> TestBot {
        let data = self
            .gql(
                &self.roles.alice,
                CREATE_BOT,
                json!({ "input": {
                    "serverId": self.roles.server_id,
                    "name": name,
                    "runBy": "The docs team",
                    "readsGoTo": "A model the docs team runs",
                }}),
            )
            .await;
        TestBot {
            id: data["createBot"]["bot"]["userId"]
                .as_str()
                .expect("id")
                .to_string(),
            token: data["createBot"]["token"]
                .as_str()
                .expect("token")
                .to_string(),
        }
    }

    /// A channel `title` the Owner creates and is in.
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

    /// A channel with `bot` and the member `bob` in it.
    async fn channel_with(&self, title: &str, private: bool, bot: &TestBot) -> String {
        let channel = self.channel(title, private).await;
        self.add(&channel, &bot.id).await;
        self.add(&channel, &self.roles.bob.user_id.to_string())
            .await;
        channel
    }

    async fn add(&self, channel: &str, user: &str) {
        self.gql(
            &self.roles.alice,
            ADD_MEMBERS,
            json!({ "s": self.roles.server_id, "c": channel, "u": [user] }),
        )
        .await;
    }

    /// `text` from `bob` into `channel`, with the message's id.
    async fn say(&self, channel: &str, text: &str) -> String {
        self.gql(
            &self.roles.bob,
            SEND,
            json!({ "input": {
                "serverId": self.roles.server_id, "conversationId": channel, "text": text,
            }}),
        )
        .await["sendMessage"]["id"]
            .as_str()
            .expect("id")
            .to_string()
    }

    async fn call(&self, token: &str, method: Method, path: &str, body: Option<Value>) -> Answer {
        let mut request = self
            .http
            .request(method, format!("{}/api/bot/v1{path}", self.node.origin))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("a call");
        Answer {
            status: response.status(),
            retry_after: response
                .headers()
                .get("retry-after")
                .map(|v| v.to_str().expect("retry-after").to_string()),
            bytes: response.text().await.expect("a body"),
        }
    }

    async fn get(&self, token: &str, path: &str) -> Answer {
        self.call(token, Method::GET, path, None).await
    }

    async fn post(&self, token: &str, channel: &str, text: &str) -> Answer {
        self.call(
            token,
            Method::POST,
            &format!("/channels/{channel}/messages"),
            Some(json!({ "text": text })),
        )
        .await
    }

    async fn audit_count(&self, actor: &str, command_id: &str) -> i64 {
        self.ctx
            .conn
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM command_audit_log \
                 WHERE actor_user_id = $1 AND command_id = $2 AND initiator = 'bot'",
                [
                    actor.parse::<uuid::Uuid>().expect("uuid").into(),
                    command_id.into(),
                ],
            ))
            .await
            .expect("audit")
            .expect("a row")
            .try_get::<i64>("", "n")
            .expect("n")
    }

    /// How many notification rows `user` has.
    async fn notification_rows(&self, user: &str) -> i64 {
        self.ctx
            .conn
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM user_notification WHERE user_id = $1",
                [user.parse::<uuid::Uuid>().expect("uuid").into()],
            ))
            .await
            .expect("notifications")
            .expect("a row")
            .try_get::<i64>("", "n")
            .expect("n")
    }

    async fn stop(self) {
        self.node.stop().await;
    }
}

fn mention_of(bot: &TestBot) -> String {
    format!("<@{}>", bot.id)
}

fn texts(messages: &Value) -> Vec<&str> {
    messages
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| m["text"].as_str().expect("text"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bot_token_works_only_here_and_a_session_cookie_works_nowhere_here() {
    let ctx = TestCtx::new().await;
    ctx.conn.execute_unprepared(RESET).await.expect("truncate");
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let node = start(&url, open_registration).await;
    let roles = ctx.roles("doors").await;
    let s = Scene {
        ctx,
        roles,
        node,
        http: reqwest::Client::new(),
    };
    let bot = s.bot("Helper").await;
    let origin = s.node.origin.clone();

    let answer: Value = s
        .http
        .post(format!("{origin}/api"))
        .bearer_auth(&bot.token)
        .json(&json!({ "query": "{ me { id } }" }))
        .send()
        .await
        .expect("graphql")
        .json()
        .await
        .expect("json");
    assert_eq!(
        answer["errors"][0]["extensions"]["code"], "UNAUTHENTICATED",
        "{answer}"
    );
    for (method, path, refused_with) in [
        (Method::GET, "/api/ws", StatusCode::UNAUTHORIZED),
        (Method::POST, "/api/files/upload", StatusCode::FORBIDDEN),
        (Method::POST, "/api/files/avatar", StatusCode::FORBIDDEN),
    ] {
        let response = s
            .http
            .request(method, format!("{origin}{path}"))
            .bearer_auth(&bot.token)
            .send()
            .await
            .expect("a route");
        assert_eq!(response.status(), refused_with, "{path}");
    }

    let (_, cookies) = key_signup(&s.http, &s.node, "person", &keys(9), json!({})).await;
    let cookie = format!(
        "rim_session={}; rim_csrf={}",
        cookies["rim_session"], cookies["rim_csrf"]
    );
    let me_url = format!("{origin}/api/bot/v1/me");
    let refused = s
        .http
        .get(&me_url)
        .header("cookie", &cookie)
        .header("x-csrf-token", &cookies["rim_csrf"])
        .send()
        .await
        .expect("a cookie alone");
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(refused.text().await.expect("body"), BAD_TOKEN_BODY);

    let with_both: Value = s
        .http
        .get(&me_url)
        .header("cookie", &cookie)
        .bearer_auth(&bot.token)
        .send()
        .await
        .expect("both")
        .json()
        .await
        .expect("json");
    assert_eq!(
        with_both["id"],
        json!(bot.id),
        "the token decides, not the cookie"
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_call_answers_for_a_channel_the_bot_was_added_to() {
    let s = scene("calls", |_| {}).await;
    let bot = s.bot("Helper").await;
    let ops = s.channel_with("ops", false, &bot).await;
    let secret = s.channel_with("secret", true, &bot).await;
    let lobby = s.channel("lobby", false).await;
    let hello = s.say(&ops, "hello world").await;
    s.say(&ops, "second word").await;
    s.say(&ops, "third word").await;
    s.say(&secret, "hidden word").await;
    s.say(&lobby, "lobby word").await;

    let me = s.get(&bot.token, "/me").await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(
        me.json(),
        json!({
            "untrusted": true,
            "id": bot.id,
            "name": "Helper",
            "workspaceId": s.roles.server_id,
            "runBy": "The docs team",
            "readsGoTo": "A model the docs team runs",
            "seesMemberJoins": false,
        })
    );

    let channels = s.get(&bot.token, "/channels").await.json();
    assert_eq!(channels["untrusted"], true);
    let mut listed: Vec<Value> = channels["channels"].as_array().expect("channels").clone();
    listed.sort_by_key(|c| c["name"].as_str().expect("name").to_string());
    assert_eq!(
        listed,
        vec![
            json!({ "id": ops, "name": "ops", "private": false }),
            json!({ "id": secret, "name": "secret", "private": true }),
        ]
    );

    let page = s
        .get(&bot.token, &format!("/channels/{ops}/messages?limit=2"))
        .await
        .json();
    assert_eq!(page["untrusted"], true);
    assert_eq!(texts(&page["messages"]), ["second word", "third word"]);
    let first = &page["messages"][0];
    assert_eq!(first["channelId"], json!(ops));
    assert_eq!(first["threadId"], Value::Null);
    assert_eq!(first["authorId"], json!(s.roles.bob.user_id.to_string()));
    assert_eq!(first["authorName"], "calls-bob");
    assert_eq!(first["authorIsBot"], false);
    assert_eq!(first["editedAt"], Value::Null);
    assert!(first["sentAt"].is_string());
    assert!(first.get("flagged").is_none() && first.get("flaggedAt").is_none());
    let cursor = page["cursor"].as_str().expect("a cursor");
    let older = s
        .get(
            &bot.token,
            &format!("/channels/{ops}/messages?limit=2&before={cursor}"),
        )
        .await
        .json();
    assert_eq!(texts(&older["messages"]), ["hello world"]);
    assert_eq!(older["messages"][0]["id"], json!(hello));
    assert_eq!(older["cursor"], Value::Null);

    let found = s
        .get(
            &bot.token,
            &format!("/channels/{ops}/search?q=word&limit=10"),
        )
        .await
        .json();
    assert_eq!(found["untrusted"], true);
    assert_eq!(found["cursor"], Value::Null, "a search has no cursor");
    let mut words = texts(&found["messages"]);
    words.sort_unstable();
    assert_eq!(words, ["second word", "third word"]);
    assert_eq!(found["messages"][0]["authorName"], "calls-bob");

    // `in:` is refused whether or not the channel it names exists, and whatever `from:` names.
    let in_lobby = s
        .get(
            &bot.token,
            &format!("/channels/{ops}/search?q=word%20in%3Alobby"),
        )
        .await;
    let in_nothing = s
        .get(
            &bot.token,
            &format!("/channels/{ops}/search?q=word%20in%3Anothing"),
        )
        .await;
    assert_eq!(in_lobby.status, StatusCode::BAD_REQUEST);
    assert_eq!(in_lobby.bytes, in_nothing.bytes);
    assert_eq!(in_lobby.json()["refusal"], "INVALID");
    let from_other = s
        .get(
            &bot.token,
            &format!("/channels/{ops}/search?q=word%20from%3Acalls-carol"),
        )
        .await;
    assert_eq!(from_other.json()["messages"], json!([]));

    let posted = s.post(&bot.token, &ops, "from the bot").await;
    assert_eq!(posted.status, StatusCode::CREATED, "{}", posted.bytes);
    let posted = posted.json();
    assert_eq!(posted["channelId"], json!(ops));
    let seen = s
        .gql(
            &s.roles.bob,
            MESSAGES,
            json!({ "s": s.roles.server_id, "c": ops }),
        )
        .await;
    let seen = seen["messages"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|m| m["id"] == posted["id"])
        .expect("a member sees it")
        .clone();
    assert_eq!(seen["text"], "from the bot");
    assert_eq!(seen["author"]["id"], json!(bot.id));
    assert_eq!(seen["author"]["displayName"], "Helper");
    assert_eq!(seen["author"]["isBot"], true);
    let own = s
        .get(&bot.token, &format!("/channels/{ops}/messages?limit=1"))
        .await
        .json();
    assert_eq!(own["messages"][0]["authorIsBot"], true);
    assert_eq!(own["messages"][0]["authorName"], "Helper");

    let mentions = s.get(&bot.token, "/mentions").await;
    assert_eq!(mentions.status, StatusCode::OK);
    assert_eq!(
        mentions.json(),
        json!({ "untrusted": true, "mentions": [], "cursor": null })
    );
    assert_eq!(s.audit_count(&bot.id, "bot.mentions").await, 0);

    s.say(&ops, &format!("{} look", mention_of(&bot))).await;
    let handed_over = s.get(&bot.token, "/mentions").await.json();
    assert_eq!(handed_over["mentions"].as_array().expect("a list").len(), 1);
    assert_eq!(s.audit_count(&bot.id, "bot.mentions").await, 1);

    for command_id in [
        "bot.me",
        "chat.list_channels",
        "chat.get_recent_messages",
        "chat.search_messages",
        "chat.post_message",
        "bot.mentions",
    ] {
        assert!(
            s.audit_count(&bot.id, command_id).await >= 1,
            "{command_id} left no audit row with the initiator bot"
        );
    }

    let unknown_call = s.get(&bot.token, "/channels/x/nothing").await;
    assert_eq!(unknown_call.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown_call.bytes, NOT_FOUND_BODY);
    let wrong_method = s.call(&bot.token, Method::DELETE, "/me", None).await;
    assert_eq!(wrong_method.status, StatusCode::METHOD_NOT_ALLOWED);
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_channel_not_joined_looks_like_one_that_does_not_exist() {
    let s = scene("hidden", |_| {}).await;
    let bot = s.bot("Helper").await;
    let joined = s.channel_with("joined", false, &bot).await;
    let public = s.channel("public", false).await;
    let private = s.channel("private", true).await;
    let elsewhere = create_channel(
        &s.node.schema,
        &s.roles.olive,
        &s.roles.other_server_id,
        "elsewhere",
        false,
    )
    .await;
    let unknown = unknown_id();

    for channel in [
        public.as_str(),
        private.as_str(),
        elsewhere.as_str(),
        unknown.as_str(),
        "not-a-uuid",
    ] {
        let calls = [
            s.get(&bot.token, &format!("/channels/{channel}/messages"))
                .await,
            s.get(&bot.token, &format!("/channels/{channel}/search?q=x"))
                .await,
            s.post(&bot.token, channel, "hello").await,
        ];
        for answer in calls {
            assert_eq!(answer.status, StatusCode::NOT_FOUND, "{channel}");
            assert_eq!(answer.bytes, NOT_FOUND_BODY, "{channel}");
        }
    }

    // A channel the bot was removed from goes back to not existing.
    s.gql(
        &s.roles.alice,
        REMOVE_MEMBER,
        json!({ "s": s.roles.server_id, "c": joined, "u": bot.id }),
    )
    .await;
    let gone = s
        .get(&bot.token, &format!("/channels/{joined}/messages"))
        .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    assert_eq!(gone.bytes, NOT_FOUND_BODY);
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bad_call_is_invalid_before_anything_runs() {
    let s = scene("invalid", |_| {}).await;
    let bot = s.bot("Helper").await;
    let ops = s.channel_with("ops", false, &bot).await;

    for path in [
        format!("/channels/{ops}/messages?limit=0"),
        format!("/channels/{ops}/messages?limit=101"),
        format!("/channels/{ops}/messages?limit=many"),
        format!("/channels/{ops}/messages?before=not-a-cursor"),
        format!("/channels/{ops}/search"),
        format!("/channels/{ops}/search?q="),
        format!("/channels/{ops}/search?q=%20%20"),
        format!("/channels/{ops}/search?q={}", "x".repeat(501)),
        format!("/channels/{ops}/search?q=x&limit=0"),
        "/mentions?limit=101".to_string(),
        "/mentions?after=12345".to_string(),
    ] {
        let answer = s.get(&bot.token, &path).await;
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(answer.json()["refusal"], "INVALID", "{path}");
        assert_eq!(answer.json()["retrySafe"], false, "{path}");
    }

    let path = format!("/channels/{ops}/messages");
    for body in [json!({}), json!({ "text": 5 }), json!(["text"])] {
        let answer = s.call(&bot.token, Method::POST, &path, Some(body)).await;
        assert_eq!(answer.status, StatusCode::BAD_REQUEST);
        assert_eq!(answer.json()["refusal"], "INVALID");
    }
    let too_large = s.post(&bot.token, &ops, &"x".repeat(128 * 1024 + 1)).await;
    assert_eq!(too_large.status, StatusCode::BAD_REQUEST);
    let too_long = s.post(&bot.token, &ops, &"x".repeat(100 * 1024)).await;
    assert_eq!(
        too_long.status,
        StatusCode::BAD_REQUEST,
        "{}",
        too_long.bytes
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn moderation_applies_to_a_bot_as_to_a_person() {
    let s = scene("gates", |_| {}).await;
    let alice = &s.roles.alice;
    let ws = &s.roles.server_id;
    let bot = s.bot("Helper").await;
    let ops = s.channel_with("ops", false, &bot).await;
    let read = format!("/channels/{ops}/messages");

    // A timeout stops writing and leaves reading.
    let until = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    s.gql(
        alice,
        TIMEOUT,
        json!({ "s": ws, "u": bot.id, "until": until }),
    )
    .await;
    let refused = s.post(&bot.token, &ops, "while timed out").await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.bytes);
    assert_eq!(refused.json()["refusal"], "DENIED");
    assert_eq!(refused.json()["retrySafe"], false);
    assert!(
        refused.json()["message"]
            .as_str()
            .expect("message")
            .contains("timed out"),
        "{}",
        refused.bytes
    );
    assert_eq!(s.get(&bot.token, &read).await.status, StatusCode::OK);
    s.gql(alice, CLEAR_TIMEOUT, json!({ "s": ws, "u": bot.id }))
        .await;

    // Slow mode counts the bot's own last message.
    s.gql(
        alice,
        SLOW_MODE,
        json!({ "s": ws, "c": ops, "seconds": 60 }),
    )
    .await;
    assert_eq!(
        s.post(&bot.token, &ops, "the first").await.status,
        StatusCode::CREATED
    );
    let slowed = s.post(&bot.token, &ops, "too soon").await;
    assert_eq!(slowed.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(slowed.json()["refusal"], "FAILED");
    assert_eq!(slowed.json()["retrySafe"], true);
    assert!(slowed.retry_after.is_some());
    s.gql(alice, SLOW_MODE, json!({ "s": ws, "c": ops, "seconds": 0 }))
        .await;

    // Posting restricted to moderators.
    s.gql(alice, POSTING, json!({ "s": ws, "c": ops })).await;
    let restricted = s.post(&bot.token, &ops, "not allowed").await;
    assert_eq!(restricted.status, StatusCode::FORBIDDEN);
    assert_eq!(restricted.json()["refusal"], "DENIED");

    // An archived channel can be read, cannot be written to and is no longer listed.
    let notes = s.channel_with("notes", false, &bot).await;
    s.gql(alice, ARCHIVE, json!({ "s": ws, "c": notes })).await;
    let archived = s.post(&bot.token, &notes, "too late").await;
    assert_eq!(archived.status, StatusCode::FORBIDDEN);
    assert_eq!(archived.json()["refusal"], "DENIED");
    assert_eq!(
        s.get(&bot.token, &format!("/channels/{notes}/messages"))
            .await
            .status,
        StatusCode::OK
    );
    let listed = s.get(&bot.token, "/channels").await.json();
    assert!(
        !listed["channels"]
            .as_array()
            .expect("channels")
            .iter()
            .any(|c| c["id"] == json!(notes))
    );

    // A refused post is on the audit log too.
    assert!(
        s.audit_count(&bot.id, "chat.post_message").await >= 2,
        "refused posts leave rows"
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_way_a_token_stops_working_looks_the_same() {
    let s = scene("stops", |_| {}).await;
    let alice = &s.roles.alice;
    let ws = &s.roles.server_id;

    let assert_refused = async |token: &str| {
        let answer = s.get(token, "/me").await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
        assert_eq!(answer.bytes, BAD_TOKEN_BODY);
    };

    assert_refused(&format!("rimbot_{}", "a".repeat(43))).await;
    assert_refused("not-a-token").await;

    let revoked = s.bot("Revoked").await;
    let tokens = s.gql(alice, TOKENS, json!({ "id": revoked.id })).await;
    s.gql(alice, REVOKE, json!({ "id": tokens["botTokens"][0]["id"] }))
        .await;
    assert_refused(&revoked.token).await;

    let deactivated = s.bot("Deactivated").await;
    s.gql(alice, DEACTIVATE, json!({ "id": deactivated.id }))
        .await;
    assert_refused(&deactivated.token).await;

    let banned = s.bot("Banned").await;
    s.gql(alice, BAN, json!({ "s": ws, "u": banned.id })).await;
    assert_refused(&banned.token).await;

    let kicked = s.bot("Kicked").await;
    s.gql(alice, KICK, json!({ "s": ws, "u": kicked.id })).await;
    assert_refused(&kicked.token).await;

    // The bots feature off refuses a token that works with it on.
    let working = s.bot("Working").await;
    assert_eq!(s.get(&working.token, "/me").await.status, StatusCode::OK);
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
    let answer = s.get(&working.token, "/me").await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(answer.bytes, BAD_TOKEN_BODY);
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bots_messages_count_against_its_own_budget() {
    let s = scene("budget", |config| {
        config.rate_messages_per_min_per_user = Some(1);
        config.rate_messages_burst = Some(2);
    })
    .await;
    let bot = s.bot("Helper").await;
    let ops = s.channel_with("ops", false, &bot).await;

    assert_eq!(
        s.post(&bot.token, &ops, "one").await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        s.post(&bot.token, &ops, "two").await.status,
        StatusCode::CREATED
    );
    let over = s.post(&bot.token, &ops, "three").await;
    assert_eq!(over.status, StatusCode::TOO_MANY_REQUESTS, "{}", over.bytes);
    assert!(over.retry_after.is_some());
    assert_eq!(over.json()["retrySafe"], true);
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mentions_are_found_once_in_order_and_only_where_the_bot_still_is() {
    let s = scene("mentions", |_| {}).await;
    let bot = s.bot("Helper").await;
    let at_bot = mention_of(&bot);
    let ops = s.channel_with("ops", false, &bot).await;
    let private = s.channel_with("private", true, &bot).await;
    let lobby = s.channel("lobby", false).await;
    s.add(&lobby, &s.roles.bob.user_id.to_string()).await;
    let vault = s.channel("vault", true).await;
    s.add(&vault, &s.roles.bob.user_id.to_string()).await;

    // Mentioning a bot where it is not a member stores nothing, public or private.
    s.say(&lobby, &format!("{at_bot} not here")).await;
    s.say(&vault, &format!("{at_bot} not here either")).await;

    // A bot mentioning itself is dropped, as for anyone.
    assert_eq!(
        s.post(&bot.token, &ops, &format!("{at_bot} myself"))
            .await
            .status,
        StatusCode::CREATED
    );

    s.say(&ops, &format!("{at_bot} first")).await;
    s.say(&private, &format!("{at_bot} second")).await;
    s.say(&ops, "no mention here").await;
    s.say(&ops, &format!("{at_bot} third")).await;

    let page = s.get(&bot.token, "/mentions?limit=2").await.json();
    assert_eq!(page["untrusted"], true);
    assert_eq!(
        texts(&page["mentions"]),
        [
            format!("{at_bot} first").as_str(),
            format!("{at_bot} second").as_str()
        ]
    );
    assert_eq!(page["mentions"][0]["authorName"], "mentions-bob");
    assert_eq!(page["mentions"][0]["authorIsBot"], false);
    assert_eq!(page["mentions"][1]["channelId"], json!(private));

    let cursor = page["cursor"].as_str().expect("a cursor").to_string();
    let next = s
        .get(&bot.token, &format!("/mentions?after={cursor}"))
        .await
        .json();
    assert_eq!(
        texts(&next["mentions"]),
        [format!("{at_bot} third").as_str()]
    );
    let cursor = next["cursor"].as_str().expect("a cursor").to_string();

    // Nothing new: the same cursor comes back, and nothing repeats.
    let empty = s
        .get(&bot.token, &format!("/mentions?after={cursor}"))
        .await
        .json();
    assert_eq!(empty["mentions"], json!([]));
    assert_eq!(empty["cursor"], json!(cursor));

    // A mention after the cursor, then deleted, does not show.
    let deleted = s.say(&ops, &format!("{at_bot} deleted")).await;
    s.gql(
        &s.roles.bob,
        DELETE,
        json!({ "input": {
            "serverId": s.roles.server_id, "conversationId": ops, "messageId": deleted,
        }}),
    )
    .await;
    let after_delete = s
        .get(&bot.token, &format!("/mentions?after={cursor}"))
        .await
        .json();
    assert_eq!(after_delete["mentions"], json!([]));

    // A channel the bot has left is left out, the rest stays.
    s.gql(
        &s.roles.alice,
        REMOVE_MEMBER,
        json!({ "s": s.roles.server_id, "c": private, "u": bot.id }),
    )
    .await;
    let without_private = s.get(&bot.token, "/mentions").await.json();
    assert_eq!(
        texts(&without_private["mentions"]),
        [
            format!("{at_bot} first").as_str(),
            format!("{at_bot} third").as_str()
        ]
    );

    // With no cursor the start is 24 hours back; with one it is the cursor.
    let oldest_cursor = without_private["mentions"][0]["id"].clone();
    s.ctx
        .conn
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE message_mention SET created_at = now() - interval '25 hours' \
             WHERE message_id = $1",
            [oldest_cursor
                .as_str()
                .expect("id")
                .parse::<uuid::Uuid>()
                .expect("uuid")
                .into()],
        ))
        .await
        .expect("age a mention");
    let recent = s.get(&bot.token, "/mentions").await.json();
    assert_eq!(
        texts(&recent["mentions"]),
        [format!("{at_bot} third").as_str()]
    );
    let long_ago = format!(
        "{}_{}",
        (chrono::Utc::now() - chrono::Duration::hours(48)).timestamp_micros(),
        uuid::Uuid::nil().simple()
    );
    let from_cursor = s
        .get(&bot.token, &format!("/mentions?after={long_ago}"))
        .await
        .json();
    assert_eq!(
        texts(&from_cursor["mentions"]),
        [
            format!("{at_bot} first").as_str(),
            format!("{at_bot} third").as_str()
        ]
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bot_gets_no_notifications_and_still_finds_its_mentions() {
    let s = scene("quiet", |_| {}).await;
    let bot = s.bot("Helper").await;
    let at_bot = mention_of(&bot);
    let ops = s.channel_with("ops", false, &bot).await;

    s.say(&ops, &format!("{at_bot} @channel look")).await;
    let posted = s.post(&bot.token, &ops, "my own message").await;
    assert_eq!(posted.status, StatusCode::CREATED);
    s.gql(
        &s.roles.bob,
        REACT,
        json!({ "input": {
            "serverId": s.roles.server_id, "conversationId": ops,
            "messageId": posted.json()["id"], "emoji": "\u{1f44d}",
        }}),
    )
    .await;

    assert_eq!(s.notification_rows(&bot.id).await, 0);
    assert!(
        s.notification_rows(&s.roles.alice.user_id.to_string())
            .await
            > 0
    );
    let mentions = s.get(&bot.token, "/mentions").await.json();
    assert_eq!(
        texts(&mentions["mentions"]),
        [format!("{at_bot} @channel look").as_str()]
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nobody_can_claim_to_be_a_bot_through_graphql() {
    let s = scene("claim", |_| {}).await;
    let channel = s.channel("ops", false).await;
    let bob = &s.roles.bob;

    let literal = s
        .node
        .schema
        .execute(bob.request(
            r#"mutation { executeCommand(input: { commandId: "chat.list_channels", initiator: BOT }) { status } }"#,
            json!({}),
        ))
        .await;
    assert!(!literal.errors.is_empty(), "{:?}", literal.data);

    let variable = s
        .node
        .schema
        .execute(bob.request(
            r#"mutation($input: ExecuteCommandInput!) { executeCommand(input: $input) { status } }"#,
            json!({ "input": {
                "commandId": "chat.post_message",
                "args": { "server_id": s.roles.server_id, "conversation_id": channel, "text": "x" },
                "initiator": "BOT",
            }}),
        ))
        .await;
    assert!(!variable.errors.is_empty(), "{:?}", variable.data);

    let in_macro = s
        .node
        .schema
        .execute(bob.request(
            r#"mutation($input: ExecuteMacroInput!) { executeMacro(input: $input) { status } }"#,
            json!({ "input": { "macroId": unknown_id(), "initiator": "BOT" } }),
        ))
        .await;
    assert!(!in_macro.errors.is_empty(), "{:?}", in_macro.data);

    assert_eq!(
        s.audit_count(&bob.user_id.to_string(), "chat.post_message")
            .await,
        0
    );
    s.stop().await;
}

/// How long a row of the feed waits, and a little more.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(2_100);

impl Scene {
    /// Let `who` allow or stop `bot` seeing members join.
    async fn set_joins(&self, who: &Caller, bot: &TestBot, on: bool) {
        self.gql(
            who,
            UPDATE_BOT,
            json!({ "input": { "botUserId": bot.id, "seesMemberJoins": on } }),
        )
        .await;
    }

    /// The cursor of a first poll, which must hand nothing over.
    async fn start_events(&self, bot: &TestBot) -> String {
        let answer = self.get(&bot.token, "/events").await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.bytes);
        let page = answer.json();
        assert_eq!(page["events"], json!([]));
        assert_eq!(page["untrusted"], true);
        page["cursor"].as_str().expect("a cursor").to_string()
    }

    /// One page of the feed after `cursor`.
    async fn events_after(&self, bot: &TestBot, cursor: &str, limit: u32) -> Value {
        let answer = self
            .get(&bot.token, &format!("/events?after={cursor}&limit={limit}"))
            .await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.bytes);
        answer.json()
    }

    /// Run `sql` with `values` on the database underneath.
    async fn sql(&self, sql: &str, values: Vec<sea_orm::Value>) {
        self.ctx
            .conn
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .expect(sql);
    }

    /// A conversation written straight to the database with `bot` a member and a message from
    /// `bob`, past every check that would refuse it.
    async fn forced_conversation(
        &self,
        server_id: &str,
        kind: &str,
        encrypted: bool,
        bot: &TestBot,
        text: &str,
    ) {
        let id = uuid::Uuid::now_v7();
        let server: uuid::Uuid = server_id.parse().expect("server");
        let member: uuid::Uuid = bot.id.parse().expect("bot");
        self.sql(
            "INSERT INTO conversation (id, conversation_type, title, server_id, encrypted_at) \
             VALUES ($1, $2::text::conversation_type, $3, $4, \
                     CASE WHEN $5::boolean THEN now() END)",
            vec![
                id.into(),
                kind.into(),
                format!("forced-{id}").into(),
                server.into(),
                encrypted.into(),
            ],
        )
        .await;
        self.sql(
            "INSERT INTO conversation_member (id, user_id, conversation_id) VALUES ($1, $2, $3)",
            vec![uuid::Uuid::now_v7().into(), member.into(), id.into()],
        )
        .await;
        self.sql(
            "INSERT INTO message (id, conversation_id, sent_by, text) VALUES ($1, $2, $3, $4)",
            vec![
                uuid::Uuid::now_v7().into(),
                id.into(),
                uuid::Uuid::from(self.roles.bob.user_id).into(),
                text.into(),
            ],
        )
        .await;
    }

    /// What the audit log holds for `command_id` by `actor`, newest first, as text.
    async fn audit_args(&self, actor: &str, command_id: &str) -> Vec<String> {
        self.ctx
            .conn
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT args_redacted::text AS args FROM command_audit_log \
                 WHERE actor_user_id = $1 AND command_id = $2 ORDER BY created_at DESC",
                [
                    actor.parse::<uuid::Uuid>().expect("uuid").into(),
                    command_id.into(),
                ],
            ))
            .await
            .expect("audit")
            .iter()
            .map(|row| row.try_get::<String>("", "args").expect("args"))
            .collect()
    }
}

/// A page's events in short: a message's text, or `+name` for a member joining.
fn seen(page: &Value) -> Vec<String> {
    page["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| match event["type"].as_str().expect("a type") {
            "message" => event["message"]["text"].as_str().expect("text").to_string(),
            "member_joined" => format!("+{}", event["user"]["name"].as_str().expect("a name")),
            other => panic!("an unknown type: {other}"),
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_feed_starts_now_and_gives_both_kinds_in_order_across_pages() {
    let s = scene("feed", |_| {}).await;
    let bot = s.bot("Helper").await;
    let ops = s.channel_with("ops", false, &bot).await;
    let other = s.channel_with("other", true, &bot).await;
    s.set_joins(&s.roles.alice, &bot, true).await;

    // Everything from before the first poll stays out, however long a later poll waits.
    s.say(&ops, "before the first poll").await;
    let start = s.start_events(&bot).await;
    assert_eq!(s.events_after(&bot, &start, 50).await["events"], json!([]));

    s.say(&ops, "one").await;
    s.ctx
        .newcomer(&s.roles.server_id, "feed-nia", ServerMemberRole::Member)
        .await;
    s.say(&other, "two").await;
    tokio::time::sleep(SETTLE).await;

    let first = s.events_after(&bot, &start, 2).await;
    assert_eq!(seen(&first), ["one", "+feed-nia"]);
    assert_eq!(first["untrusted"], true);
    let event = &first["events"][0];
    assert_eq!(event["channelId"], json!(ops));
    assert_eq!(event["mentioned"], false);
    assert_eq!(event["message"]["authorName"], "feed-bob");
    assert_eq!(event["message"]["authorIsBot"], false);
    let joined = &first["events"][1];
    assert_eq!(joined["user"]["isBot"], false);
    assert!(joined["at"].is_string());
    let mut fields: Vec<&String> = joined["user"].as_object().expect("user").keys().collect();
    fields.sort();
    assert_eq!(fields, ["id", "isBot", "name"]);

    let cursor = first["cursor"].as_str().expect("a cursor");
    let second = s.events_after(&bot, cursor, 2).await;
    assert_eq!(seen(&second), ["two"]);
    assert_eq!(second["events"][0]["channelId"], json!(other));

    // Nothing new: the same cursor comes back, and nothing repeats.
    let cursor = second["cursor"].as_str().expect("a cursor");
    let empty = s.events_after(&bot, cursor, 2).await;
    assert_eq!(empty["events"], json!([]));
    assert_eq!(empty["cursor"], json!(cursor));

    // An old cursor still pages forward from where it stood.
    assert_eq!(
        seen(&s.events_after(&bot, &start, 50).await),
        ["one", "+feed-nia", "two"]
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rows_tied_across_both_tables_are_each_given_once() {
    let s = scene("tied", |_| {}).await;
    let bot = s.bot("Helper").await;
    let ops = s.channel_with("ops", false, &bot).await;
    s.set_joins(&s.roles.alice, &bot, true).await;
    let start = s.start_events(&bot).await;

    let first = s.say(&ops, "first").await;
    let second = s.say(&ops, "second").await;
    let nia = s
        .ctx
        .newcomer(&s.roles.server_id, "tied-nia", ServerMemberRole::Member)
        .await;
    let omar = s
        .ctx
        .newcomer(&s.roles.server_id, "tied-omar", ServerMemberRole::Member)
        .await;
    let server: uuid::Uuid = s.roles.server_id.parse().expect("server");

    // One instant for all four rows, after the cursor and soon old enough.
    tokio::time::sleep(SETTLE).await;
    let tied = chrono::Utc::now() - chrono::Duration::seconds(1);
    let tied = sea_orm::prelude::DateTimeWithTimeZone::from(tied);
    for id in [&first, &second] {
        s.sql(
            "UPDATE message SET sent_at = $1 WHERE id = $2",
            vec![tied.into(), id.parse::<uuid::Uuid>().expect("id").into()],
        )
        .await;
    }
    for who in [&nia, &omar] {
        s.sql(
            "UPDATE server_member SET joined_at = $1 WHERE server_id = $2 AND user_id = $3",
            vec![
                tied.into(),
                server.into(),
                uuid::Uuid::from(who.user_id).into(),
            ],
        )
        .await;
    }
    tokio::time::sleep(SETTLE).await;

    let mut cursor = start;
    let mut walked = Vec::new();
    loop {
        let page = s.events_after(&bot, &cursor, 1).await;
        let found = seen(&page);
        if found.is_empty() {
            break;
        }
        assert_eq!(found.len(), 1);
        walked.extend(found);
        cursor = page["cursor"].as_str().expect("a cursor").to_string();
    }
    walked.sort();
    assert_eq!(walked, ["+tied-nia", "+tied-omar", "first", "second"]);
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_comes_as_the_messages_route_has_it_with_the_same_ones_left_out() {
    let s = scene("parity", |_| {}).await;
    let bot = s.bot("Helper").await;
    let ops = s.channel_with("ops", false, &bot).await;
    let at_bot = mention_of(&bot);
    let start = s.start_events(&bot).await;

    let plain = s.say(&ops, "plain").await;
    s.say(&ops, &format!("{at_bot} look")).await;
    s.gql(
        &s.roles.bob,
        REPLY,
        json!({ "input": {
            "serverId": s.roles.server_id,
            "conversationId": ops,
            "rootMessageId": plain,
            "text": format!("{at_bot} in a thread"),
        }}),
    )
    .await;
    let deleted = s.say(&ops, "deleted").await;
    s.gql(
        &s.roles.bob,
        DELETE,
        json!({ "input": {
            "serverId": s.roles.server_id, "conversationId": ops, "messageId": deleted,
        }}),
    )
    .await;
    assert_eq!(
        s.post(&bot.token, &ops, "the bot's own").await.status,
        StatusCode::CREATED
    );
    tokio::time::sleep(SETTLE).await;

    let page = s.events_after(&bot, &start, 50).await;
    let look = format!("{at_bot} look");
    assert_eq!(seen(&page), ["plain", look.as_str()]);
    assert_eq!(page["events"][0]["mentioned"], false);
    assert_eq!(page["events"][1]["mentioned"], true);

    // The same message reads the same on the route for the channel, which also has no thread
    // reply or deleted message.
    let listed = s
        .get(&bot.token, &format!("/channels/{ops}/messages?limit=100"))
        .await
        .json();
    let listed: Vec<&Value> = listed["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|message| message["authorId"] != json!(bot.id))
        .collect();
    assert_eq!(listed.len(), 2);
    assert_eq!(&page["events"][0]["message"], listed[0]);
    assert_eq!(&page["events"][1]["message"], listed[1]);

    // The mention in a thread is on `/mentions`, and not in the feed.
    let mentions = s.get(&bot.token, "/mentions").await.json();
    let in_a_thread = format!("{at_bot} in a thread");
    assert!(texts(&mentions["mentions"]).contains(&in_a_thread.as_str()));
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_younger_than_two_seconds_waits_for_a_later_poll() {
    let s = scene("hold", |_| {}).await;
    let bot = s.bot("Helper").await;
    let ops = s.channel_with("ops", false, &bot).await;
    let start = s.start_events(&bot).await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let id = s.say(&ops, "fresh").await;
    assert_eq!(s.events_after(&bot, &start, 50).await["events"], json!([]));

    let id: uuid::Uuid = id.parse().expect("id");
    s.sql(
        "UPDATE message SET sent_at = now() - interval '1 second' WHERE id = $1",
        vec![id.into()],
    )
    .await;
    assert_eq!(s.events_after(&bot, &start, 50).await["events"], json!([]));

    s.sql(
        "UPDATE message SET sent_at = now() - interval '2500 milliseconds' WHERE id = $1",
        vec![id.into()],
    )
    .await;
    assert_eq!(seen(&s.events_after(&bot, &start, 50).await), ["fresh"]);
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_channel_counts_while_the_bot_is_in_it_and_never_from_before_it_was_added() {
    let s = scene("member", |_| {}).await;
    let bot = s.bot("Helper").await;
    let ops = s.channel_with("ops", false, &bot).await;
    let later = s.channel("later", true).await;
    s.add(&later, &s.roles.bob.user_id.to_string()).await;
    let shut = s.channel_with("shut", false, &bot).await;
    let start = s.start_events(&bot).await;

    s.say(&later, "before the bot was added").await;
    s.say(&ops, "ops, while it is in").await;
    s.say(&shut, "in a channel archived later").await;
    s.gql(
        &s.roles.alice,
        ARCHIVE,
        json!({ "s": s.roles.server_id, "c": shut }),
    )
    .await;
    tokio::time::sleep(SETTLE).await;
    s.add(&later, &bot.id).await;
    s.say(&later, "after the bot was added").await;
    tokio::time::sleep(SETTLE).await;

    let page = s.events_after(&bot, &start, 50).await;
    assert_eq!(
        seen(&page),
        ["ops, while it is in", "after the bot was added"]
    );

    // Removed: what it did not read yet stops at once, and so does what comes later.
    s.say(&ops, "ops, just before it left").await;
    tokio::time::sleep(SETTLE).await;
    s.gql(
        &s.roles.alice,
        REMOVE_MEMBER,
        json!({ "s": s.roles.server_id, "c": ops, "u": bot.id }),
    )
    .await;
    s.say(&ops, "ops, after it left").await;
    tokio::time::sleep(SETTLE).await;
    let cursor = page["cursor"].as_str().expect("a cursor");
    assert_eq!(s.events_after(&bot, cursor, 50).await["events"], json!([]));
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_but_a_plaintext_channel_of_its_workspace_reaches_the_feed() {
    let s = scene("forced", |_| {}).await;
    let bot = s.bot("Helper").await;
    let start = s.start_events(&bot).await;
    let here = s.roles.server_id.clone();
    let there = s.roles.other_server_id.clone();
    s.forced_conversation(&here, "direct", false, &bot, "a direct message")
        .await;
    s.forced_conversation(&here, "group", false, &bot, "a group")
        .await;
    s.forced_conversation(&here, "channel", true, &bot, "encrypted")
        .await;
    s.forced_conversation(&there, "channel", false, &bot, "another workspace")
        .await;
    s.forced_conversation(&here, "channel", false, &bot, "a real channel")
        .await;
    tokio::time::sleep(SETTLE).await;

    assert_eq!(
        seen(&s.events_after(&bot, &start, 50).await),
        ["a real channel"]
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn joins_need_the_setting_and_a_bot_of_another_workspace_sees_nothing_of_this_one() {
    let s = scene("joins", |_| {}).await;
    let bot = s.bot("Helper").await;
    let start = s.start_events(&bot).await;
    let me = s.get(&bot.token, "/me").await.json();
    assert_eq!(me["seesMemberJoins"], false);

    let elsewhere = s
        .gql(
            &s.roles.olive,
            CREATE_BOT,
            json!({ "input": {
                "serverId": s.roles.other_server_id,
                "name": "Elsewhere",
                "runBy": "Olive",
                "readsGoTo": "Nowhere",
                "seesMemberJoins": true,
            }}),
        )
        .await;
    let elsewhere = TestBot {
        id: elsewhere["createBot"]["bot"]["userId"]
            .as_str()
            .expect("id")
            .to_string(),
        token: elsewhere["createBot"]["token"]
            .as_str()
            .expect("token")
            .to_string(),
    };
    assert_eq!(
        s.get(&elsewhere.token, "/me").await.json()["seesMemberJoins"],
        true
    );
    let elsewhere_start = s.start_events(&elsewhere).await;

    let off = s
        .ctx
        .newcomer(&s.roles.server_id, "joins-off", ServerMemberRole::Member)
        .await;
    tokio::time::sleep(SETTLE).await;
    assert_eq!(s.events_after(&bot, &start, 50).await["events"], json!([]));

    // A Member cannot turn it on; a Moderator can, and any member sees that it is on.
    let refused = exec_err_code(
        &s.node.schema,
        s.roles.bob.request(
            UPDATE_BOT,
            json!({ "input": { "botUserId": bot.id, "seesMemberJoins": true } }),
        ),
    )
    .await;
    assert_eq!(refused, "FORBIDDEN");
    s.set_joins(&s.roles.mona, &bot, true).await;
    assert_eq!(
        s.get(&bot.token, "/me").await.json()["seesMemberJoins"],
        true
    );
    let listed = s
        .gql(&s.roles.bob, LIST_BOTS, json!({ "s": s.roles.server_id }))
        .await;
    assert_eq!(listed["bots"][0]["seesMemberJoins"], true);
    let recorded = s
        .audit_args(&s.roles.mona.user_id.to_string(), "bot.update")
        .await;
    assert!(
        recorded[0].contains("\"sees_member_joins\": true"),
        "{recorded:?}"
    );

    // A bot joining counts, marked as one; so does a member from another server.
    let helper = s.bot("Second").await;
    let guest = s
        .ctx
        .newcomer(&s.roles.server_id, "joins-remote", ServerMemberRole::Member)
        .await;
    s.sql(
        r#"UPDATE "user" SET kind = 'remote' WHERE id = $1"#,
        vec![uuid::Uuid::from(guest.user_id).into()],
    )
    .await;
    tokio::time::sleep(SETTLE).await;
    let page = s.events_after(&bot, &start, 50).await;
    assert_eq!(seen(&page), ["+joins-off", "+Second", "+joins-remote"]);
    assert_eq!(
        page["events"][0]["user"]["id"],
        json!(off.user_id.to_string())
    );
    assert_eq!(page["events"][1]["user"]["isBot"], true);
    assert_eq!(page["events"][1]["user"]["id"], json!(helper.id));
    assert_eq!(page["events"][2]["user"]["isBot"], false);

    // Nobody who was there before the bot, even from a cursor forged far back, and never itself.
    let forged = format!("0_{}", uuid::Uuid::nil().simple());
    let from_forged = s.events_after(&bot, &forged, 50).await;
    assert_eq!(
        seen(&from_forged),
        ["+joins-off", "+Second", "+joins-remote"]
    );

    // A kick and a return is a new row, so a new event.
    let cursor = page["cursor"].as_str().expect("a cursor").to_string();
    s.gql(
        &s.roles.alice,
        KICK,
        json!({ "s": s.roles.server_id, "u": off.user_id.to_string() }),
    )
    .await;
    s.ctx
        .join(&s.roles.server_id, &off, ServerMemberRole::Member)
        .await;
    tokio::time::sleep(SETTLE).await;
    assert_eq!(
        seen(&s.events_after(&bot, &cursor, 50).await),
        ["+joins-off"]
    );

    // Turned off: nothing more, at once.
    s.set_joins(&s.roles.alice, &bot, false).await;
    assert_eq!(s.events_after(&bot, &cursor, 50).await["events"], json!([]));

    // The other workspace's bot saw none of it.
    assert_eq!(
        s.events_after(&elsewhere, &elsewhere_start, 50).await["events"],
        json!([])
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_feed_is_recorded_with_counts_and_a_bad_call_is_invalid() {
    let s = scene("record", |_| {}).await;
    let bot = s.bot("Helper").await;
    let ops = s.channel_with("ops", false, &bot).await;
    s.set_joins(&s.roles.alice, &bot, true).await;
    let start = s.start_events(&bot).await;
    assert_eq!(s.events_after(&bot, &start, 50).await["events"], json!([]));
    assert_eq!(s.audit_count(&bot.id, "bot.events").await, 0);

    s.say(&ops, "a very private sentence").await;
    s.ctx
        .newcomer(&s.roles.server_id, "record-nia", ServerMemberRole::Member)
        .await;
    tokio::time::sleep(SETTLE).await;
    assert_eq!(
        s.events_after(&bot, &start, 50).await["events"]
            .as_array()
            .expect("events")
            .len(),
        2
    );
    assert_eq!(s.audit_count(&bot.id, "bot.events").await, 1);
    let recorded = s.audit_args(&bot.id, "bot.events").await;
    assert!(recorded[0].contains("\"messages\": 1"), "{recorded:?}");
    assert!(recorded[0].contains("\"member_joined\": 1"), "{recorded:?}");
    assert!(!recorded[0].contains("private"), "{recorded:?}");
    assert!(!recorded[0].contains("record-nia"), "{recorded:?}");

    assert_eq!(
        s.events_after(&bot, &start, 1).await["events"]
            .as_array()
            .expect("events")
            .len(),
        1
    );
    assert_eq!(
        s.events_after(&bot, &start, 100).await["events"]
            .as_array()
            .expect("events")
            .len(),
        2
    );
    for path in [
        "/events?after=nope",
        "/events?after=12_",
        "/events?limit=0",
        "/events?limit=101",
        "/events?limit=many",
    ] {
        let answer = s.get(&bot.token, path).await;
        assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(answer.json()["refusal"], "INVALID");
    }
    let unknown = s.get("rimbot_nope", "/events").await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.bytes, BAD_TOKEN_BODY);
    s.stop().await;
}
