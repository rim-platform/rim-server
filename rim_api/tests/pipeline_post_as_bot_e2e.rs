//! A pipeline's Post as bot action on a real server: who may save one, what it posts when someone
//! joins, each reason it posts nothing, and how a client that does not know the action sees and
//! saves it.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests --test pipeline_post_as_bot_e2e -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::time::Duration;

use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};

use rim_server::config::RIMConfig;

use support::graphql::{Caller, exec_err, exec_ok};
use support::nodes::{Node, S2S_TABLES, start};
use support::relay::eventually;
use support::roles::{Roles, create_channel};
use support::{TestCtx, unknown_id};

const CREATE_BOT: &str = r#"mutation($input: CreateBotInput!) {
  createBot(input: $input) { bot { userId } }
}"#;
const DEACTIVATE_BOT: &str = "mutation($id: ID!) { deactivateBot(botUserId: $id) { userId } }";
const ADD_MEMBERS: &str = r#"mutation($s: ID!, $c: ID!, $u: [ID!]!) {
  addConversationMembers(serverId: $s, conversationId: $c, userIds: $u) { memberIds }
}"#;
const REMOVE_MEMBER: &str = r#"mutation($s: ID!, $c: ID!, $u: ID!) {
  removeConversationMember(serverId: $s, conversationId: $c, userId: $u)
}"#;
const ARCHIVE: &str = r#"mutation($s: ID!, $c: ID!) {
  setConversationArchived(serverId: $s, conversationId: $c, archived: true) { id }
}"#;
const SET_ROLE: &str = r#"mutation($s: ID!, $u: ID!, $r: ServerMemberRoleGql!) {
  setMemberRole(serverId: $s, userId: $u, role: $r) { memberRole }
}"#;
const CREATE_INVITE: &str = r#"mutation($input: CreateInviteInput!) {
  createInvite(input: $input) { inviteToken }
}"#;
const JOIN: &str = r#"mutation($input: JoinServerByInviteInput!) {
  joinServerByInvite(input: $input) { serverId }
}"#;
const MESSAGES: &str = r#"query($s: ID!, $c: ID!) {
  messages(serverId: $s, conversationId: $c, dir: AFTER, limit: 50, includeDeleted: false) {
    items { id text author { id isBot } }
  }
}"#;
const ACTION_FIELDS: &str = "actionType commandId args postAsBot { botUserId conversationId text }";
/// What a client from before the action selects.
const OLD_LIST: &str = r#"query($s: ID!) {
  pipelines(serverId: $s) { id actions { actionType commandId args } }
}"#;
const LAST_FIRED: &str = "query($s: ID!) { pipelines(serverId: $s) { id lastFiredAt } }";

/// A workspace with its people on a running server, with a channel, a bot in it and a way to tell
/// when an event has been evaluated.
struct Scene {
    ctx: TestCtx,
    roles: Roles,
    node: Node,
    channel: String,
    bot: String,
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
    let channel = create_channel(
        &node.schema,
        &roles.alice,
        &roles.server_id,
        "welcome",
        false,
    )
    .await;
    let mut scene = Scene {
        ctx,
        roles,
        node,
        channel,
        bot: String::new(),
    };
    scene.bot = scene.new_bot("Greeter").await;
    let (channel, bot) = (scene.channel.clone(), scene.bot.clone());
    scene.add(&channel, &bot).await;
    scene
}

impl Scene {
    async fn ok(&self, who: &Caller, query: &str, variables: Value) -> Value {
        exec_ok(&self.node.schema, who.request(query, variables)).await
    }

    async fn err(&self, who: &Caller, query: &str, variables: Value) -> (String, String) {
        exec_err(&self.node.schema, who.request(query, variables)).await
    }

    async fn new_bot(&self, name: &str) -> String {
        self.ok(
            &self.roles.alice,
            CREATE_BOT,
            json!({ "input": {
                "serverId": self.roles.server_id,
                "name": name,
                "runBy": "Ada",
                "readsGoTo": "Nowhere",
            }}),
        )
        .await["createBot"]["bot"]["userId"]
            .as_str()
            .expect("id")
            .to_string()
    }

    async fn add(&self, channel: &str, user: &str) {
        self.ok(
            &self.roles.alice,
            ADD_MEMBERS,
            json!({ "s": self.roles.server_id, "c": channel, "u": [user] }),
        )
        .await;
    }

    /// The `actions` entry of a Post as bot action.
    fn post_action(&self, bot: &str, channel: &str, text: &str) -> Value {
        json!({
            "commandId": "chat.post_message",
            "postAsBot": { "botUserId": bot, "conversationId": channel, "text": text },
        })
    }

    /// `createPipeline` input for `actions` on `kind`.
    fn input(&self, kind: &str, actions: Value) -> Value {
        json!({ "input": {
            "serverId": self.roles.server_id,
            "name": "Welcome",
            "trigger": { "kind": kind },
            "actions": actions,
        }})
    }

    /// A welcome pipeline `who` creates with `text`, with its id.
    async fn welcome(&self, who: &Caller, text: &str) -> String {
        self.welcome_with(who, text, json!({})).await
    }

    /// As [`Self::welcome`], with `extra` input fields.
    async fn welcome_with(&self, who: &Caller, text: &str, extra: Value) -> String {
        let action = self.post_action(&self.bot, &self.channel, text);
        let mut variables = self.input("MEMBER_JOINED", json!([action]));
        for (key, value) in extra.as_object().expect("extra") {
            variables["input"][key] = value.clone();
        }
        let query = format!(
            "mutation($input: CreatePipelineInput!) {{ createPipeline(input: $input) {{ id actions {{ {ACTION_FIELDS} }} }} }}"
        );
        self.ok(who, &query, variables).await["createPipeline"]["id"]
            .as_str()
            .expect("id")
            .to_string()
    }

    /// A new person with a session, joining through a real invite.
    async fn join(&self, name: &str) -> Caller {
        let who = self.ctx.outsider(name).await;
        self.join_as(&who).await;
        who
    }

    async fn join_as(&self, who: &Caller) {
        let token = self
            .ok(
                &self.roles.alice,
                CREATE_INVITE,
                json!({ "input": { "serverId": self.roles.server_id } }),
            )
            .await["createInvite"]["inviteToken"]
            .as_str()
            .expect("token")
            .to_string();
        self.ok(who, JOIN, json!({ "input": { "inviteToken": token } }))
            .await;
    }

    /// The messages of `channel` from bots, as `[author id, text]`.
    async fn bot_posts(&self, channel: &str) -> Vec<(String, String)> {
        self.ok(
            &self.roles.alice,
            MESSAGES,
            json!({ "s": self.roles.server_id, "c": channel }),
        )
        .await["messages"]["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter(|message| message["author"]["isBot"] == true)
            .map(|message| {
                (
                    message["author"]["id"].as_str().expect("id").to_string(),
                    message["text"].as_str().expect("text").to_string(),
                )
            })
            .collect()
    }

    /// The first bot post in the scene's channel, once there is one.
    async fn first_post(&self) -> (String, String) {
        let channel = self.channel.clone();
        eventually("the bot's post", || {
            let channel = channel.clone();
            async move { self.bot_posts(&channel).await.into_iter().next() }
        })
        .await
    }

    /// When `who`'s pipeline `id` last fired.
    async fn last_fired(&self, who: &Caller, id: &str) -> Value {
        self.ok(who, LAST_FIRED, json!({ "s": self.roles.server_id }))
            .await["pipelines"]
            .as_array()
            .expect("pipelines")
            .iter()
            .find(|pipeline| pipeline["id"] == json!(id))
            .expect("the pipeline")["lastFiredAt"]
            .clone()
    }

    /// Wait until `who`'s pipeline `id` has fired since `before`, so the join was evaluated.
    async fn fired_since(&self, who: &Caller, id: &str, before: &Value) {
        eventually("the pipeline firing", || async {
            let now = self.last_fired(who, id).await;
            (now != Value::Null && now != *before).then_some(())
        })
        .await;
    }

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

    async fn count(&self, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
        self.ctx
            .conn
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .expect(sql)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saving_refuses_whoever_and_whatever_could_not_post() {
    let s = scene("saving", |_| {}).await;
    let (alice, bob) = (&s.roles.alice, &s.roles.bob);
    const CREATE: &str =
        "mutation($input: CreatePipelineInput!) { createPipeline(input: $input) { id } }";
    let good = |text: &str| s.post_action(&s.bot, &s.channel, text);

    // A Member may make other pipelines, not this action.
    let (code, _) = s
        .err(bob, CREATE, s.input("MEMBER_JOINED", json!([good("Hi")])))
        .await;
    assert_eq!(code, "FORBIDDEN");

    for kind in ["NEW_MESSAGE", "MESSAGE_UPDATED", "REACTIONS_UPDATED"] {
        let (code, message) = s
            .err(alice, CREATE, s.input(kind, json!([good("Hi")])))
            .await;
        assert_eq!(code, "VALIDATION", "{kind}");
        assert!(message.contains("Member Joined"), "{message}");
    }

    // The text: a typo in a placeholder is named, and nothing else in braces is taken.
    let (code, message) = s
        .err(
            alice,
            CREATE,
            s.input("MEMBER_JOINED", json!([good("Hi {usernme}")])),
        )
        .await;
    assert_eq!(code, "VALIDATION");
    assert!(message.contains("{usernme}"), "{message}");
    for text in ["", "   ", "{}", "{name} {other}"] {
        let (code, _) = s
            .err(alice, CREATE, s.input("MEMBER_JOINED", json!([good(text)])))
            .await;
        assert_eq!(code, "VALIDATION", "{text:?}");
    }
    let too_long = "x".repeat(10_001);
    let (code, _) = s
        .err(
            alice,
            CREATE,
            s.input("MEMBER_JOINED", json!([good(&too_long)])),
        )
        .await;
    assert_eq!(code, "VALIDATION");

    // The shape: this command alone, and no args beside it.
    for bad in [
        json!({ "commandId": "state.get", "postAsBot": good("Hi")["postAsBot"] }),
        json!({ "commandId": "chat.post_message", "args": { "x": 1 }, "postAsBot": good("Hi")["postAsBot"] }),
    ] {
        let (code, _) = s
            .err(alice, CREATE, s.input("MEMBER_JOINED", json!([bad])))
            .await;
        assert_eq!(code, "VALIDATION");
    }

    // The bot: unknown, of another workspace, deactivated.
    let elsewhere = s
        .ok(
            &s.roles.olive,
            CREATE_BOT,
            json!({ "input": {
                "serverId": s.roles.other_server_id,
                "name": "Elsewhere", "runBy": "Olive", "readsGoTo": "Nowhere",
            }}),
        )
        .await["createBot"]["bot"]["userId"]
        .as_str()
        .expect("id")
        .to_string();
    let gone = s.new_bot("Gone").await;
    s.add(&s.channel, &gone).await;
    s.ok(alice, DEACTIVATE_BOT, json!({ "id": gone })).await;
    for bot in [unknown_id(), elsewhere, gone] {
        let action = s.post_action(&bot, &s.channel, "Hi");
        let (code, message) = s
            .err(alice, CREATE, s.input("MEMBER_JOINED", json!([action])))
            .await;
        assert_eq!(code, "VALIDATION", "{message}");
        assert!(message.contains("active bot"), "{message}");
    }

    // The bot isn't added for you.
    let empty = create_channel(&s.node.schema, alice, &s.roles.server_id, "empty", false).await;
    let action = s.post_action(&s.bot, &empty, "Hi");
    let (code, message) = s
        .err(alice, CREATE, s.input("MEMBER_JOINED", json!([action])))
        .await;
    assert_eq!(code, "VALIDATION");
    assert!(message.contains("add it there first"), "{message}");

    // The channel: archived, encrypted, a direct message, unknown.
    let archived = create_channel(&s.node.schema, alice, &s.roles.server_id, "old", false).await;
    s.add(&archived, &s.bot).await;
    s.ok(
        alice,
        ARCHIVE,
        json!({ "s": s.roles.server_id, "c": archived }),
    )
    .await;
    let encrypted =
        create_channel(&s.node.schema, alice, &s.roles.server_id, "sealed", false).await;
    s.add(&encrypted, &s.bot).await;
    s.sql(
        "UPDATE conversation SET encrypted_at = now() WHERE id = $1",
        vec![uuid(&encrypted)],
    )
    .await;
    let direct = create_channel(&s.node.schema, alice, &s.roles.server_id, "direct", false).await;
    s.add(&direct, &s.bot).await;
    s.sql(
        "UPDATE conversation SET conversation_type = 'direct' WHERE id = $1",
        vec![uuid(&direct)],
    )
    .await;
    for channel in [archived, encrypted, direct, unknown_id()] {
        let action = s.post_action(&s.bot, &channel, "Hi");
        let (code, message) = s
            .err(alice, CREATE, s.input("MEMBER_JOINED", json!([action])))
            .await;
        assert_eq!(code, "VALIDATION", "{channel}");
        assert!(message.contains("plaintext channel"), "{message}");
    }

    // A private channel the person saving isn't in reads like one that doesn't exist.
    let private = create_channel(&s.node.schema, alice, &s.roles.server_id, "private", true).await;
    s.add(&private, &s.bot).await;
    let action = s.post_action(&s.bot, &private, "Hi");
    let (code, message) = s
        .err(
            &s.roles.mona,
            CREATE,
            s.input("MEMBER_JOINED", json!([action.clone()])),
        )
        .await;
    assert_eq!(code, "VALIDATION");
    assert!(message.contains("plaintext channel"), "{message}");
    s.ok(alice, CREATE, s.input("MEMBER_JOINED", json!([action])))
        .await;

    // Other pipelines stay open to anyone, on any trigger.
    s.ok(
        bob,
        CREATE,
        s.input(
            "NEW_MESSAGE",
            json!([{ "commandId": "state.get", "args": { "server_id": s.roles.server_id, "key": "k" } }]),
        ),
    )
    .await;
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_update_needs_the_same_authority_to_add_or_change_one() {
    let s = scene("update", |_| {}).await;
    let (alice, mona, bob, carol) = (&s.roles.alice, &s.roles.mona, &s.roles.bob, &s.roles.carol);
    const UPDATE: &str =
        "mutation($input: UpdatePipelineInput!) { updatePipeline(input: $input) { id name } }";
    let update = |id: &str, name: &str, trigger: &str, actions: Value| {
        json!({ "input": {
            "pipelineId": id, "name": name, "trigger": { "kind": trigger }, "actions": actions,
        }})
    };
    let post = |text: &str| s.post_action(&s.bot, &s.channel, text);

    // Someone who may not manage it, whatever they send.
    let by_mona = s.welcome(mona, "Hi {name}").await;
    let (code, _) = s
        .err(
            bob,
            UPDATE,
            update(
                &by_mona,
                "Mine",
                "MEMBER_JOINED",
                json!([post("Hi {name}")]),
            ),
        )
        .await;
    assert_eq!(code, "FORBIDDEN");

    // A Member can't add one to their own pipeline.
    let plain = json!([{ "commandId": "state.get", "args": { "server_id": s.roles.server_id, "key": "k" } }]);
    let by_carol = s
        .ok(
            carol,
            "mutation($input: CreatePipelineInput!) { createPipeline(input: $input) { id } }",
            s.input("MEMBER_JOINED", plain),
        )
        .await["createPipeline"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let (code, _) = s
        .err(
            carol,
            UPDATE,
            update(&by_carol, "Mine", "MEMBER_JOINED", json!([post("Hi")])),
        )
        .await;
    assert_eq!(code, "FORBIDDEN");

    // A Moderator can't add one to a Member's pipeline: it would never run.
    let (code, message) = s
        .err(
            alice,
            UPDATE,
            update(&by_carol, "Mine", "MEMBER_JOINED", json!([post("Hi")])),
        )
        .await;
    assert_eq!(code, "VALIDATION");
    assert!(message.contains("creator"), "{message}");

    // An Owner may change a Moderator's, which stays the Moderator's.
    s.ok(
        alice,
        UPDATE,
        update(
            &by_mona,
            "Changed",
            "MEMBER_JOINED",
            json!([post("Hello {name}")]),
        ),
    )
    .await;

    // The trigger can't move away from under it.
    let (code, _) = s
        .err(
            alice,
            UPDATE,
            update(
                &by_mona,
                "Changed",
                "NEW_MESSAGE",
                json!([post("Hello {name}")]),
            ),
        )
        .await;
    assert_eq!(code, "VALIDATION");

    // A creator no longer a Moderator may keep it unchanged, and may not change it.
    s.ok(
        alice,
        SET_ROLE,
        json!({ "s": s.roles.server_id, "u": mona.user_id.to_string(), "r": "MEMBER" }),
    )
    .await;
    s.ok(
        mona,
        UPDATE,
        update(
            &by_mona,
            "Renamed",
            "MEMBER_JOINED",
            json!([post("Hello {name}")]),
        ),
    )
    .await;
    let (code, _) = s
        .err(
            mona,
            UPDATE,
            update(
                &by_mona,
                "Renamed",
                "MEMBER_JOINED",
                json!([post("Different")]),
            ),
        )
        .await;
    assert_eq!(code, "FORBIDDEN");
    s.ok(mona, UPDATE, update(&by_mona, "Without it", "MEMBER_JOINED", json!([{ "commandId": "state.get", "args": { "server_id": s.roles.server_id, "key": "k" } }])))
        .await;
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_join_posts_once_as_the_bot_with_no_text_in_the_audit_log() {
    let s = scene("fires", |_| {}).await;
    let id = s
        .welcome(&s.roles.alice, "Welcome, {name} ({username})!")
        .await;

    let nia = s.join("fires-nia").await;
    let (author, text) = s.first_post().await;
    assert_eq!(author, s.bot);
    assert_eq!(text, "Welcome, fires-nia (fires-nia)!");
    s.fired_since(&s.roles.alice, &id, &Value::Null).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(s.bot_posts(&s.channel).await.len(), 1);
    assert_ne!(nia.user_id.to_string(), s.bot);

    assert_eq!(
        s.count(
            "SELECT count(*) AS n FROM command_audit_log WHERE actor_user_id = $1 \
             AND command_id = 'chat.post_message' AND initiator = 'bot'",
            vec![uuid(&s.bot)],
        )
        .await,
        1,
    );
    assert_eq!(
        s.count(
            "SELECT count(*) AS n FROM command_audit_log WHERE actor_user_id = $1 \
             AND args_redacted::text LIKE '%Welcome%'",
            vec![uuid(&s.bot)],
        )
        .await,
        0,
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_name_written_by_whoever_joins_mentions_nobody_and_injects_nothing() {
    let s = scene("hostile", |_| {}).await;
    let id = s
        .welcome(&s.roles.alice, "@here hello {name}, <@{username}>")
        .await;
    let hostile = format!(
        "@channel <@{}> **b**\u{202e}\n{{name}}",
        s.roles.bob.user_id
    );
    s.add(&s.channel, &s.roles.bob.user_id.to_string()).await;
    let joiner = s.ctx.outsider("hostile-nia").await;
    s.sql(
        r#"UPDATE "user" SET display_name = $1 WHERE id = $2"#,
        vec![hostile.into(), uuid::Uuid::from(joiner.user_id).into()],
    )
    .await;
    s.join_as(&joiner).await;

    let (_, text) = s.first_post().await;
    s.fired_since(&s.roles.alice, &id, &Value::Null).await;

    for hidden in ['\u{202e}', '\n'] {
        assert!(!text.contains(hidden), "{text:?}");
    }
    assert!(!text.to_lowercase().contains("@channel"), "{text:?}");
    assert!(!text.to_lowercase().contains("@here"), "{text:?}");
    assert!(!text.contains("<@"), "{text:?}");
    assert!(text.contains("\\*\\*b\\*\\*"), "{text:?}");
    assert!(
        text.contains("{name}"),
        "a name is not filled in again: {text:?}"
    );

    // Nobody was mentioned, and anyone told hears of it only as of any other message.
    assert_eq!(
        s.count(
            "SELECT count(*) AS n FROM message_mention mm JOIN message m ON m.id = mm.message_id \
             WHERE m.sent_by = $1",
            vec![uuid(&s.bot)],
        )
        .await,
        0,
    );
    assert_eq!(
        s.count(
            "SELECT count(*) AS n FROM user_notification un JOIN message m ON m.id = un.message_id \
             WHERE m.sent_by = $1 AND un.reason <> 'all'",
            vec![uuid(&s.bot)],
        )
        .await,
        0,
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_posts_nothing_once_its_authority_or_its_target_is_gone() {
    /// What is taken away before the join.
    #[derive(Debug, Clone, Copy)]
    enum Taken {
        CreatorDemoted,
        BotDeactivated,
        BotRemoved,
        ChannelArchived,
        ChannelEncrypted,
    }
    for taken in [
        Taken::CreatorDemoted,
        Taken::BotDeactivated,
        Taken::BotRemoved,
        Taken::ChannelArchived,
        Taken::ChannelEncrypted,
    ] {
        let label = format!("{taken:?}").to_lowercase();
        let s = scene(&label, |_| {}).await;
        let (alice, mona) = (&s.roles.alice, &s.roles.mona);
        let id = s.welcome(mona, "Hello {name}").await;
        match taken {
            Taken::CreatorDemoted => {
                s.ok(
                    alice,
                    SET_ROLE,
                    json!({ "s": s.roles.server_id, "u": mona.user_id.to_string(), "r": "MEMBER" }),
                )
                .await;
            }
            Taken::BotDeactivated => {
                s.ok(alice, DEACTIVATE_BOT, json!({ "id": s.bot })).await;
            }
            Taken::BotRemoved => {
                s.ok(
                    alice,
                    REMOVE_MEMBER,
                    json!({ "s": s.roles.server_id, "c": s.channel, "u": s.bot }),
                )
                .await;
            }
            Taken::ChannelArchived => {
                s.ok(
                    alice,
                    ARCHIVE,
                    json!({ "s": s.roles.server_id, "c": s.channel }),
                )
                .await;
            }
            Taken::ChannelEncrypted => {
                s.sql(
                    "UPDATE conversation SET encrypted_at = now() WHERE id = $1",
                    vec![uuid(&s.channel)],
                )
                .await;
            }
        }
        s.join(&format!("{label}-nia")).await;
        s.fired_since(mona, &id, &Value::Null).await;
        assert_eq!(s.bot_posts(&s.channel).await, vec![], "{taken:?}");
        s.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bot_joining_spends_nothing_and_the_cooldown_skips_rather_than_queues() {
    let s = scene("burst", |_| {}).await;
    let id = s
        .welcome_with(
            &s.roles.alice,
            "Hello {name}",
            json!({ "cooldownSecs": 3600 }),
        )
        .await;

    // A bot joining fires nothing, so it leaves the cooldown for the next person.
    s.new_bot("Second").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(s.bot_posts(&s.channel).await, vec![]);
    assert_eq!(s.last_fired(&s.roles.alice, &id).await, Value::Null);

    s.join("burst-nia").await;
    let (_, text) = s.first_post().await;
    assert_eq!(text, "Hello burst-nia");

    // Inside the cooldown the next one is skipped, and it never comes later.
    s.join("burst-omar").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(s.bot_posts(&s.channel).await.len(), 1);
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_fires_past_a_bot_in_a_pipeline_that_also_runs_a_command() {
    let s = scene("mixed", |_| {}).await;
    const CREATE: &str =
        "mutation($input: CreatePipelineInput!) { createPipeline(input: $input) { id } }";
    let actions = json!([
        s.post_action(&s.bot, &s.channel, "Hello {name}"),
        { "commandId": "state.get", "args": { "server_id": s.roles.server_id, "key": "k" } },
    ]);
    let id = s
        .ok(&s.roles.alice, CREATE, s.input("MEMBER_JOINED", actions))
        .await["createPipeline"]["id"]
        .as_str()
        .expect("id")
        .to_string();

    // The command still runs for a bot joining; the bot's post does not.
    s.new_bot("Second").await;
    s.fired_since(&s.roles.alice, &id, &Value::Null).await;
    assert_eq!(s.bot_posts(&s.channel).await, vec![]);
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_does_not_know_the_action_lists_and_saves_it_without_changing_it() {
    let s = scene("older", |_| {}).await;
    let alice = &s.roles.alice;
    let id = s.welcome(alice, "Hello {name}").await;

    // It reads a command with its arguments, from the fields it has always selected.
    let listed = s
        .ok(alice, OLD_LIST, json!({ "s": s.roles.server_id }))
        .await;
    let action = listed["pipelines"][0]["actions"][0].clone();
    assert_eq!(action["actionType"], "PostAsBot");
    assert_eq!(action["commandId"], "chat.post_message");
    assert_eq!(action["args"]["text"], "Hello {name}");
    assert_eq!(action["args"]["conversation_id"], json!(s.channel));
    assert_eq!(action["args"]["bot_user_id"], json!(s.bot));

    // Saving what it was given keeps the action, under a new name.
    let update = format!(
        "mutation($input: UpdatePipelineInput!) {{ updatePipeline(input: $input) {{ name actions {{ {ACTION_FIELDS} }} }} }}"
    );
    let saved = |name: &str, actions: Value| {
        json!({ "input": {
            "pipelineId": id, "name": name, "trigger": { "kind": "MEMBER_JOINED" }, "actions": actions,
        }})
    };
    let sent_back = json!([{ "commandId": action["commandId"], "args": action["args"] }]);
    let kept = s.ok(alice, &update, saved("Renamed", sent_back)).await;
    let kept = &kept["updatePipeline"];
    assert_eq!(kept["name"], "Renamed");
    assert_eq!(kept["actions"][0]["actionType"], "PostAsBot");
    assert_eq!(kept["actions"][0]["postAsBot"]["text"], "Hello {name}");
    assert_eq!(kept["actions"][0]["postAsBot"]["botUserId"], json!(s.bot));

    // Changing it there is refused, so it never turns into a command run as its creator.
    let mut changed = action["args"].clone();
    changed["text"] = json!("Something else");
    let (code, message) = s
        .err(
            alice,
            &update,
            saved(
                "Renamed",
                json!([{ "commandId": "chat.post_message", "args": changed }]),
            ),
        )
        .await;
    assert_eq!(code, "VALIDATION");
    assert!(message.contains("Post as bot"), "{message}");
    let (code, _) = s
        .err(
            alice,
            "mutation($input: CreatePipelineInput!) { createPipeline(input: $input) { id } }",
            s.input(
                "MEMBER_JOINED",
                json!([{ "commandId": "chat.post_message", "args": action["args"] }]),
            ),
        )
        .await;
    assert_eq!(code, "VALIDATION");

    // It still fires as the bot, and the client can still remove it.
    s.join("older-nia").await;
    let (author, text) = s.first_post().await;
    assert_eq!(author, s.bot);
    assert_eq!(text, "Hello older-nia");
    let removed = s
        .ok(
            alice,
            &update,
            saved(
                "Without it",
                json!([{ "commandId": "state.get", "args": { "server_id": s.roles.server_id, "key": "k" } }]),
            ),
        )
        .await;
    assert_eq!(
        removed["updatePipeline"]["actions"][0]["actionType"],
        "ExecuteCommand"
    );
    assert_eq!(
        removed["updatePipeline"]["actions"][0]["postAsBot"],
        Value::Null
    );
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_member_sees_the_action_and_older_stored_actions_still_read() {
    let s = scene("stored", |_| {}).await;
    s.welcome(&s.roles.alice, "Hello {name}").await;
    let list = format!(
        "query($s: ID!) {{ pipelines(serverId: $s) {{ id actions {{ {ACTION_FIELDS} }} }} }}"
    );
    let seen = s
        .ok(&s.roles.bob, &list, json!({ "s": s.roles.server_id }))
        .await;
    let post = &seen["pipelines"][0]["actions"][0]["postAsBot"];
    assert_eq!(post["text"], "Hello {name}");
    assert_eq!(post["conversationId"], json!(s.channel));

    // A row written before this action existed.
    s.sql(
        "INSERT INTO pipeline (id, server_id, name, trigger_data, actions_data, enabled, created_by) \
         VALUES ($1, $2, 'Old', $3, $4, false, $5)",
        vec![
            uuid::Uuid::now_v7().into(),
            uuid(&s.roles.server_id),
            json!({ "kind": "NewMessage", "conversation_id": null }).into(),
            json!([{ "type": "ExecuteCommand", "command_id": "state.get", "args": null }]).into(),
            uuid::Uuid::from(s.roles.alice.user_id).into(),
        ],
    )
    .await;
    let seen = s
        .ok(&s.roles.bob, &list, json!({ "s": s.roles.server_id }))
        .await;
    let pipelines = seen["pipelines"].as_array().expect("pipelines");
    assert_eq!(pipelines.len(), 2);
    assert!(pipelines.iter().any(|pipeline| {
        pipeline["actions"][0]["actionType"] == "ExecuteCommand"
            && pipeline["actions"][0]["commandId"] == "state.get"
    }));
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saving_one_is_refused_while_bots_are_off() {
    use rim_core::server_feature::model::ServerFeature;

    let s = scene("off", |_| {}).await;
    let schema = s.ctx.build_schema_with_disabled(&[ServerFeature::Bots]);
    let action = s.post_action(&s.bot, &s.channel, "Hi");
    let (code, _) = exec_err(
        &schema,
        s.roles.alice.request(
            "mutation($input: CreatePipelineInput!) { createPipeline(input: $input) { id } }",
            s.input("MEMBER_JOINED", json!([action])),
        ),
    )
    .await;
    assert_eq!(code, "FEATURE_DISABLED");
    s.stop().await;
}
