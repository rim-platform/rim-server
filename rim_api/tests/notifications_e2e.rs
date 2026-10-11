//! Through-the-stack e2e tests for the server-side notification rules.
//!
//! Every assertion goes through GraphQL: resolver → `NotificationService` / `MessageService` /
//! `ReadStateService` / `CommandService` → SeaORM repositories → Postgres. Notifications are
//! created synchronously after a message commits, so they are visible as soon as `sendMessage`
//! returns. The `NotificationDue` event's `quiet` flag is covered by unit tests (the e2e schema
//! uses a no-op broadcaster); here the stored row is checked.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::server_member::model::ServerMemberRole;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{Caller, exec_err_code, exec_ok};
use support::roles::create_channel;

const CREATE_CONVERSATION: &str = r#"
mutation Create($input: CreateConversationInput!) {
  conversation: createConversation(input: $input) { id }
}
"#;

const JOIN_CHANNEL: &str = r#"
mutation Join($serverId: ID!, $conversationId: ID!) {
  conversation: joinChannel(serverId: $serverId, conversationId: $conversationId) { id }
}
"#;

const ADD_MEMBERS: &str = r#"
mutation Add($serverId: ID!, $conversationId: ID!, $userIds: [ID!]!) {
  conversation: addConversationMembers(serverId: $serverId, conversationId: $conversationId,
                                       userIds: $userIds) { id }
}
"#;

const REMOVE_MEMBER: &str = r#"
mutation Remove($serverId: ID!, $conversationId: ID!, $userId: ID!) {
  removeConversationMember(serverId: $serverId, conversationId: $conversationId, userId: $userId)
}
"#;

const SEND_MESSAGE: &str = r#"
mutation Send($input: SendMessageInput!) { msg: sendMessage(input: $input) { id } }
"#;

const REPLY: &str = r#"
mutation Reply($input: ReplyToMessageInput!) { msg: replyToMessage(input: $input) { id } }
"#;

const DELETE_MESSAGE: &str = r#"
mutation Delete($input: DeleteMessageInput!) { deleteMessage(input: $input) }
"#;

const MARK_AS_READ: &str = r#"
mutation MarkRead($input: MarkAsReadInput!) { markAsRead(input: $input) }
"#;

const MUTE: &str = r#"
mutation Mute($input: MuteConversationInput!) { muteConversation(input: $input) }
"#;

const UNMUTE: &str = r#"
mutation Unmute($input: UnmuteConversationInput!) { unmuteConversation(input: $input) }
"#;

const CONVERSATION_LEVEL: &str = r#"
query Conversation($serverId: ID!, $conversationId: ID!) {
  conversation(serverId: $serverId, conversationId: $conversationId) {
    id isMuted notificationLevel effectiveNotificationLevel
  }
}
"#;

const SET_LEVEL: &str = r#"
mutation SetLevel($serverId: ID!, $conversationId: ID!, $level: NotificationLevel) {
  level: setConversationNotificationLevel(serverId: $serverId, conversationId: $conversationId,
                                          level: $level) {
    conversationId level effectiveLevel mutedUntil
  }
}
"#;

const SETTINGS: &str = r#"
query Settings($serverId: ID!) {
  settings: myNotificationSettings(serverId: $serverId) {
    serverId defaultLevel keywords dnd { start end timezone } dndPaused dndPausedUntil digest
  }
}
"#;

const UPDATE_SETTINGS: &str = r#"
mutation Update($serverId: ID!, $input: UpdateNotificationSettingsInput!) {
  settings: updateNotificationSettings(serverId: $serverId, input: $input) {
    serverId defaultLevel keywords dnd { start end timezone } dndPaused dndPausedUntil digest
  }
}
"#;

const SET_DND: &str = r#"
mutation Dnd($serverId: ID!, $paused: Boolean!, $until: DateTime) {
  settings: setDoNotDisturb(serverId: $serverId, paused: $paused, until: $until) {
    dndPaused dndPausedUntil
  }
}
"#;

const NOTIFICATIONS: &str = r#"
query Notifications($serverId: ID!, $unreadOnly: Boolean, $dir: PageDir,
                    $cursor: NotificationCursorInput, $limit: Int) {
  page: myNotifications(serverId: $serverId, unreadOnly: $unreadOnly, dir: $dir,
                        cursor: $cursor, limit: $limit) {
    items {
      id conversationId messageId reason createdAt readAt
      conversation { id title }
      message { id text author { username } }
    }
    next { createdAt notificationId }
    prev { createdAt notificationId }
    pageInfo { hasNext hasPrev }
  }
}
"#;

const UNREAD_COUNT: &str = r#"
query Count($serverId: ID!) { count: myUnreadNotificationCount(serverId: $serverId) }
"#;

const MARK_NOTIFICATIONS_READ: &str = r#"
mutation MarkNotificationsRead($serverId: ID!, $ids: [ID!], $allBefore: DateTime) {
  changed: markNotificationsRead(serverId: $serverId, ids: $ids, allBefore: $allBefore)
}
"#;

const EXECUTE: &str = r#"
mutation Exec($commandId: String!, $args: JSON, $context: CommandContextInput) {
  result: executeCommand(input: { commandId: $commandId, args: $args, commandContext: $context }) {
    status message payload
  }
}
"#;

struct Fixture {
    ctx: TestCtx,
    schema: AppSchema,
    server_id: String,
    /// Workspace owner.
    alice: Caller,
    bob: Caller,
    carol: Caller,
    /// A public channel created by alice that bob and carol joined.
    general: String,
}

async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let server = ctx.create_server("notifications-e2e").await;
    let alice_user = ctx.create_user("nt-alice").await;
    let bob_user = ctx.create_user("nt-bob").await;
    let carol_user = ctx.create_user("nt-carol").await;
    ctx.add_owner(&server, &alice_user).await;
    ctx.add_member(&server, &bob_user, ServerMemberRole::Member)
        .await;
    ctx.add_member(&server, &carol_user, ServerMemberRole::Member)
        .await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let mut f = Fixture {
        alice: ctx.caller(&alice_user).await,
        bob: ctx.caller(&bob_user).await,
        carol: ctx.caller(&carol_user).await,
        server_id: server.id.to_string(),
        schema,
        ctx,
        general: String::new(),
    };
    let alice = f.alice;
    f.general = create_channel(&f.schema, &alice, &f.server_id, "general", false).await;
    let (bob, carol, general) = (f.bob, f.carol, f.general.clone());
    f.join(&bob, &general).await;
    f.join(&carol, &general).await;
    f
}

fn mention(who: &Caller) -> String {
    format!("<@{}>", who.user_id)
}

impl Fixture {
    fn ids(&self, conversation_id: &str) -> Value {
        json!({ "serverId": self.server_id, "conversationId": conversation_id })
    }

    async fn create_dm(&self, who: &Caller, other: &Caller) -> String {
        let data = exec_ok(
            &self.schema,
            who.request(
                CREATE_CONVERSATION,
                json!({ "input": {
                    "serverId": self.server_id, "otherUserId": other.user_id.to_string(),
                } }),
            ),
        )
        .await;
        data["conversation"]["id"].as_str().expect("id").to_string()
    }

    async fn join(&self, who: &Caller, conversation_id: &str) {
        exec_ok(
            &self.schema,
            who.request(JOIN_CHANNEL, self.ids(conversation_id)),
        )
        .await;
    }

    async fn send(&self, who: &Caller, conversation_id: &str, text: &str) -> String {
        let data = exec_ok(
            &self.schema,
            who.request(
                SEND_MESSAGE,
                json!({ "input": {
                    "serverId": self.server_id, "conversationId": conversation_id, "text": text,
                } }),
            ),
        )
        .await;
        data["msg"]["id"].as_str().expect("id").to_string()
    }

    async fn notifications_page(&self, who: &Caller, vars: Value) -> Value {
        let mut vars = vars;
        vars["serverId"] = json!(self.server_id);
        exec_ok(&self.schema, who.request(NOTIFICATIONS, vars)).await["page"].clone()
    }

    /// All of `who`'s notifications, newest first.
    async fn notifications(&self, who: &Caller) -> Vec<Value> {
        self.notifications_page(who, json!({ "limit": 100 })).await["items"]
            .as_array()
            .expect("items")
            .clone()
    }

    /// `(messageId, reason)` of `who`'s notifications, newest first.
    async fn reasons(&self, who: &Caller) -> Vec<(String, String)> {
        self.notifications(who)
            .await
            .iter()
            .map(|n| {
                (
                    n["messageId"].as_str().expect("messageId").to_string(),
                    n["reason"].as_str().expect("reason").to_string(),
                )
            })
            .collect()
    }

    async fn unread_count(&self, who: &Caller) -> i64 {
        exec_ok(
            &self.schema,
            who.request(UNREAD_COUNT, json!({ "serverId": self.server_id })),
        )
        .await["count"]
            .as_i64()
            .expect("count")
    }

    async fn set_level(&self, who: &Caller, conversation_id: &str, level: Value) -> Value {
        let mut vars = self.ids(conversation_id);
        vars["level"] = level;
        exec_ok(&self.schema, who.request(SET_LEVEL, vars)).await["level"].clone()
    }

    async fn update_settings(&self, who: &Caller, input: Value) -> Value {
        exec_ok(
            &self.schema,
            who.request(
                UPDATE_SETTINGS,
                json!({ "serverId": self.server_id, "input": input }),
            ),
        )
        .await["settings"]
            .clone()
    }

    async fn update_settings_err(&self, who: &Caller, input: Value) -> String {
        exec_err_code(
            &self.schema,
            who.request(
                UPDATE_SETTINGS,
                json!({ "serverId": self.server_id, "input": input }),
            ),
        )
        .await
    }

    async fn conversation_level(&self, who: &Caller, conversation_id: &str) -> Value {
        exec_ok(
            &self.schema,
            who.request(CONVERSATION_LEVEL, self.ids(conversation_id)),
        )
        .await["conversation"]
            .clone()
    }

    async fn mark_read(
        &self,
        who: &Caller,
        ids: Option<Vec<String>>,
        all_before: Option<String>,
    ) -> i64 {
        exec_ok(
            &self.schema,
            who.request(
                MARK_NOTIFICATIONS_READ,
                json!({ "serverId": self.server_id, "ids": ids, "allBefore": all_before }),
            ),
        )
        .await["changed"]
            .as_i64()
            .expect("changed")
    }

    async fn execute(
        &self,
        who: &Caller,
        command_id: &str,
        args: Value,
        conversation: Option<&str>,
    ) -> Value {
        exec_ok(
            &self.schema,
            who.request(
                EXECUTE,
                json!({
                    "commandId": command_id,
                    "args": args,
                    "context": { "workspaceId": self.server_id, "conversationId": conversation },
                }),
            ),
        )
        .await["result"]
            .clone()
    }
}

/// Settings `input` with defaults for the fields a test does not care about.
fn settings_input(default_level: &str, keywords: &[&str], digest: &str) -> Value {
    json!({ "defaultLevel": default_level, "keywords": keywords, "dnd": null, "digest": digest })
}

/// Alice sets `MENTIONS` on #general: bob's plain message creates nothing for her, his `@alice`
/// does. Carol, on the default `ALL`, is told about both.
#[tokio::test]
async fn mentions_level_admits_mentions_only() {
    let f = setup().await;
    let level = f.set_level(&f.alice, &f.general, json!("MENTIONS")).await;
    assert_eq!(level["level"], "MENTIONS");
    assert_eq!(level["effectiveLevel"], "MENTIONS");
    let conv = f.conversation_level(&f.alice, &f.general).await;
    assert_eq!(conv["notificationLevel"], "MENTIONS");
    assert_eq!(conv["effectiveNotificationLevel"], "MENTIONS");
    assert_eq!(conv["isMuted"], false);
    let carol_view = f.conversation_level(&f.carol, &f.general).await;
    assert_eq!(carol_view["notificationLevel"], Value::Null);
    assert_eq!(carol_view["effectiveNotificationLevel"], "ALL");

    let plain = f.send(&f.bob, &f.general, "hello everyone").await;
    assert!(f.reasons(&f.alice).await.is_empty());

    let ping = f
        .send(
            &f.bob,
            &f.general,
            &format!("{} can you look?", mention(&f.alice)),
        )
        .await;
    assert_eq!(
        f.reasons(&f.alice).await,
        vec![(ping.clone(), "MENTION".to_string())]
    );
    assert_eq!(
        f.reasons(&f.carol).await,
        vec![(ping, "ALL".to_string()), (plain, "ALL".to_string())]
    );
    // The author is never notified about their own message.
    assert!(f.reasons(&f.bob).await.is_empty());

    // `level: null` goes back to the workspace default.
    let level = f.set_level(&f.alice, &f.general, Value::Null).await;
    assert_eq!(level["level"], Value::Null);
    assert_eq!(level["effectiveLevel"], "ALL");
}

#[tokio::test]
async fn channel_mention_and_keyword_hits_pass_the_mentions_level() {
    let f = setup().await;
    let saved = f
        .update_settings(
            &f.carol,
            settings_input("MENTIONS", &[" Deploy ", "on-call"], "OFF"),
        )
        .await;
    assert_eq!(saved["defaultLevel"], "MENTIONS");
    assert_eq!(saved["keywords"], json!(["deploy", "on-call"]));

    let hit = f
        .send(&f.bob, &f.general, "Deploy finished, all green")
        .await;
    f.send(&f.bob, &f.general, "deployment notes are in the doc")
        .await;
    let everyone = f.send(&f.bob, &f.general, "@channel standup in 5").await;
    assert_eq!(
        f.reasons(&f.carol).await,
        vec![
            (everyone, "MENTION".to_string()),
            (hit, "KEYWORD".to_string())
        ]
    );
}

#[tokio::test]
async fn direct_messages_are_direct_even_under_mentions() {
    let f = setup().await;
    f.update_settings(&f.alice, settings_input("MENTIONS", &[], "OFF"))
        .await;
    let dm = f.create_dm(&f.bob, &f.alice).await;
    let msg = f.send(&f.bob, &dm, "quick question").await;
    assert_eq!(f.reasons(&f.alice).await, vec![(msg, "DIRECT".to_string())]);
}

#[tokio::test]
async fn none_level_and_mutes_yield_nothing() {
    let f = setup().await;
    f.set_level(&f.alice, &f.general, json!("NONE")).await;
    f.send(&f.bob, &f.general, &format!("{} urgent", mention(&f.alice)))
        .await;
    f.send(&f.bob, &f.general, "@here anyone?").await;
    assert!(f.reasons(&f.alice).await.is_empty(), "NONE means none");
    let conv = f.conversation_level(&f.alice, &f.general).await;
    assert_eq!(conv["isMuted"], true, "an explicit NONE reads as muted");

    // muteConversation / unmuteConversation keep their shape and interplay with the level.
    let unmuted = exec_ok(
        &f.schema,
        f.alice
            .request(UNMUTE, json!({ "input": f.ids(&f.general) })),
    )
    .await;
    assert_eq!(unmuted["unmuteConversation"], true);
    let conv = f.conversation_level(&f.alice, &f.general).await;
    assert_eq!(conv["notificationLevel"], Value::Null);
    assert_eq!(conv["isMuted"], false);

    let input = json!({
        "serverId": f.server_id, "conversationId": f.general,
        "mutedUntil": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
    });
    let muted = exec_ok(&f.schema, f.alice.request(MUTE, json!({ "input": input }))).await;
    assert_eq!(muted["muteConversation"], true);
    let conv = f.conversation_level(&f.alice, &f.general).await;
    assert_eq!(conv["isMuted"], true);
    assert_eq!(conv["effectiveNotificationLevel"], "NONE");
    f.send(
        &f.bob,
        &f.general,
        &format!("{} still there?", mention(&f.alice)),
    )
    .await;
    assert!(
        f.reasons(&f.alice).await.is_empty(),
        "a timed mute silences"
    );

    exec_ok(
        &f.schema,
        f.alice
            .request(UNMUTE, json!({ "input": f.ids(&f.general) })),
    )
    .await;
    let back = f.send(&f.bob, &f.general, "welcome back").await;
    assert_eq!(f.reasons(&f.alice).await, vec![(back, "ALL".to_string())]);
}

/// Inside the do-not-disturb window (or pause) the notification is still stored; the `quiet`
/// flag on the realtime event is unit-tested.
#[tokio::test]
async fn dnd_still_records_notifications() {
    let f = setup().await;
    let now = chrono::Utc::now();
    let start = (now - chrono::Duration::hours(1))
        .format("%H:%M")
        .to_string();
    let end = (now + chrono::Duration::hours(1))
        .format("%H:%M")
        .to_string();
    let saved = f
        .update_settings(
            &f.alice,
            json!({
                "defaultLevel": "ALL", "keywords": [], "digest": "HOURLY",
                "dnd": { "start": start, "end": end, "timezone": "UTC" },
            }),
        )
        .await;
    assert_eq!(
        saved["dnd"],
        json!({ "start": start, "end": end, "timezone": "UTC" })
    );
    assert_eq!(saved["digest"], "HOURLY");
    let msg = f.send(&f.bob, &f.general, "late night thought").await;
    assert_eq!(f.reasons(&f.alice).await, vec![(msg, "ALL".to_string())]);

    // The manual pause is separate from the window and survives a settings save.
    let paused = exec_ok(
        &f.schema,
        f.alice.request(
            SET_DND,
            json!({ "serverId": f.server_id, "paused": true, "until": null }),
        ),
    )
    .await["settings"]
        .clone();
    assert_eq!(paused["dndPaused"], true);
    assert_eq!(paused["dndPausedUntil"], Value::Null);
    f.update_settings(&f.alice, settings_input("ALL", &[], "OFF"))
        .await;
    let settings = exec_ok(
        &f.schema,
        f.alice
            .request(SETTINGS, json!({ "serverId": f.server_id })),
    )
    .await["settings"]
        .clone();
    assert_eq!(settings["dndPaused"], true);
    assert_eq!(settings["dnd"], Value::Null);
    let quiet = f.send(&f.bob, &f.general, "while paused").await;
    assert_eq!(f.reasons(&f.alice).await[0], (quiet, "ALL".to_string()));

    // `until` in the past is refused; `until` without `paused` too.
    let past = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let code = exec_err_code(
        &f.schema,
        f.alice.request(
            SET_DND,
            json!({ "serverId": f.server_id, "paused": true, "until": past }),
        ),
    )
    .await;
    assert_eq!(code, "VALIDATION");
    let code = exec_err_code(
        &f.schema,
        f.alice.request(
            SET_DND,
            json!({ "serverId": f.server_id, "paused": false, "until": past }),
        ),
    )
    .await;
    assert_eq!(code, "VALIDATION");
}

#[tokio::test]
async fn settings_defaults_and_validation() {
    let f = setup().await;
    let settings = exec_ok(
        &f.schema,
        f.carol
            .request(SETTINGS, json!({ "serverId": f.server_id })),
    )
    .await["settings"]
        .clone();
    assert_eq!(
        settings,
        json!({
            "serverId": f.server_id, "defaultLevel": "ALL", "keywords": [], "dnd": null,
            "dndPaused": false, "dndPausedUntil": null, "digest": "OFF",
        })
    );

    assert_eq!(
        f.update_settings_err(&f.carol, settings_input("ALL", &["x"], "OFF"))
            .await,
        "VALIDATION"
    );
    let too_many: Vec<String> = (0..21).map(|i| format!("word{i}")).collect();
    let too_many: Vec<&str> = too_many.iter().map(String::as_str).collect();
    assert_eq!(
        f.update_settings_err(&f.carol, settings_input("ALL", &too_many, "OFF"))
            .await,
        "VALIDATION"
    );
    for dnd in [
        json!({ "start": "22:00", "end": "07:00", "timezone": "Mars/Base" }),
        json!({ "start": "25:00", "end": "07:00", "timezone": "UTC" }),
        json!({ "start": "08:00", "end": "08:00", "timezone": "UTC" }),
    ] {
        let code = f
            .update_settings_err(
                &f.carol,
                json!({ "defaultLevel": "ALL", "keywords": [], "digest": "OFF", "dnd": dnd }),
            )
            .await;
        assert_eq!(code, "VALIDATION", "{dnd}");
    }
}

#[tokio::test]
async fn notifications_page_and_mark_read() {
    let f = setup().await;
    let mut sent = vec![];
    for i in 0..5 {
        sent.push(f.send(&f.bob, &f.general, &format!("message {i}")).await);
    }
    assert_eq!(f.unread_count(&f.alice).await, 5);

    // Newest first, two at a time.
    let mut seen = vec![];
    let mut cursor = Value::Null;
    loop {
        let page = f
            .notifications_page(
                &f.alice,
                json!({ "limit": 2, "dir": "AFTER", "cursor": cursor }),
            )
            .await;
        for item in page["items"].as_array().expect("items") {
            seen.push(item["messageId"].as_str().expect("id").to_string());
            assert_eq!(item["conversation"]["title"], "general");
            assert_eq!(item["message"]["author"]["username"], "nt-bob");
            assert!(item["readAt"].is_null());
        }
        if page["next"].is_null() {
            assert_eq!(page["pageInfo"]["hasNext"], false);
            break;
        }
        assert_eq!(page["pageInfo"]["hasNext"], true);
        cursor = page["next"].clone();
    }
    let mut newest_first = sent.clone();
    newest_first.reverse();
    assert_eq!(seen, newest_first);

    // Mark the newest read by id: the unread filter and count follow.
    let items = f.notifications(&f.alice).await;
    let newest = items[0]["id"].as_str().expect("id").to_string();
    assert_eq!(
        f.mark_read(&f.alice, Some(vec![newest.clone()]), None)
            .await,
        1
    );
    assert_eq!(f.mark_read(&f.alice, Some(vec![newest]), None).await, 0);
    assert_eq!(f.unread_count(&f.alice).await, 4);
    let unread = f
        .notifications_page(&f.alice, json!({ "unreadOnly": true, "limit": 50 }))
        .await;
    assert_eq!(unread["items"].as_array().expect("items").len(), 4);

    // Someone else's notification id is ignored.
    let carol_items = f.notifications(&f.carol).await;
    let carols = carol_items[0]["id"].as_str().expect("id").to_string();
    assert_eq!(f.mark_read(&f.alice, Some(vec![carols]), None).await, 0);
    assert_eq!(f.unread_count(&f.carol).await, 5);

    // All before now.
    let now = chrono::Utc::now().to_rfc3339();
    assert_eq!(f.mark_read(&f.alice, None, Some(now)).await, 4);
    assert_eq!(f.unread_count(&f.alice).await, 0);

    // Exactly one of `ids` / `allBefore`.
    for vars in [
        json!({ "serverId": f.server_id }),
        json!({ "serverId": f.server_id, "ids": [], "allBefore": chrono::Utc::now().to_rfc3339() }),
    ] {
        let code = exec_err_code(&f.schema, f.alice.request(MARK_NOTIFICATIONS_READ, vars)).await;
        assert_eq!(code, "VALIDATION");
    }
}

#[tokio::test]
async fn reading_the_conversation_clears_its_notifications() {
    let f = setup().await;
    let other = create_channel(&f.schema, &f.alice, &f.server_id, "random", false).await;
    f.join(&f.bob, &other).await;
    f.send(&f.bob, &f.general, "one").await;
    f.send(&f.bob, &f.general, "two").await;
    f.send(&f.bob, &other, "elsewhere").await;
    assert_eq!(f.unread_count(&f.alice).await, 3);

    exec_ok(
        &f.schema,
        f.alice.request(
            MARK_AS_READ,
            json!({ "input": {
                "workspaceId": f.server_id, "conversationId": f.general,
                "readAt": chrono::Utc::now().to_rfc3339(),
            } }),
        ),
    )
    .await;
    assert_eq!(
        f.unread_count(&f.alice).await,
        1,
        "only #random stays unread"
    );
}

/// A private channel never notifies a workspace member outside it, not even for `@channel`
/// or a direct `<@user>` mention.
#[tokio::test]
async fn private_channel_never_notifies_non_members() {
    let f = setup().await;
    let secret = create_channel(&f.schema, &f.alice, &f.server_id, "secret", true).await;
    exec_ok(
        &f.schema,
        f.alice.request(
            ADD_MEMBERS,
            json!({
                "serverId": f.server_id, "conversationId": secret,
                "userIds": [f.bob.user_id.to_string()],
            }),
        ),
    )
    .await;
    let msg = f
        .send(
            &f.bob,
            &secret,
            &format!(
                "@channel and {} and {}",
                mention(&f.carol),
                mention(&f.alice)
            ),
        )
        .await;
    assert!(f.reasons(&f.carol).await.is_empty());
    assert_eq!(
        f.reasons(&f.alice).await,
        vec![(msg, "MENTION".to_string())]
    );

    // Once removed, an earlier notification stays in the inbox but no longer resolves the
    // conversation or the message.
    let earlier = f.send(&f.alice, &secret, "for bob").await;
    let bob_items = f.notifications(&f.bob).await;
    assert_eq!(bob_items[0]["messageId"], json!(earlier));
    assert_eq!(bob_items[0]["message"]["text"], "for bob");
    exec_ok(
        &f.schema,
        f.alice.request(
            REMOVE_MEMBER,
            json!({
                "serverId": f.server_id, "conversationId": secret,
                "userId": f.bob.user_id.to_string(),
            }),
        ),
    )
    .await;
    let bob_items = f.notifications(&f.bob).await;
    assert_eq!(bob_items[0]["conversation"], Value::Null);
    assert_eq!(bob_items[0]["message"], Value::Null);
}

#[tokio::test]
async fn thread_replies_reach_participants_under_mentions() {
    let f = setup().await;
    f.set_level(&f.alice, &f.general, json!("MENTIONS")).await;
    f.set_level(&f.carol, &f.general, json!("MENTIONS")).await;
    let root = f.send(&f.alice, &f.general, "proposal: move standup").await;
    let reply = exec_ok(
        &f.schema,
        f.bob.request(
            REPLY,
            json!({ "input": {
                "serverId": f.server_id, "conversationId": f.general,
                "rootMessageId": root, "text": "fine by me",
            } }),
        ),
    )
    .await["msg"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    assert_eq!(
        f.reasons(&f.alice).await,
        vec![(reply.clone(), "THREAD_REPLY".to_string())]
    );
    assert!(
        f.reasons(&f.carol).await.is_empty(),
        "carol never took part"
    );
    let alice_items = f.notifications(&f.alice).await;
    assert_eq!(alice_items[0]["message"]["text"], "fine by me");
}

#[tokio::test]
async fn deleting_a_message_drops_its_unread_notifications() {
    let f = setup().await;
    let msg = f.send(&f.bob, &f.general, "oops, wrong channel").await;
    let keep = f.send(&f.bob, &f.general, "this one stays").await;
    let alice_items = f.notifications(&f.alice).await;
    let alice_keep = alice_items
        .iter()
        .find(|n| n["messageId"] == json!(keep))
        .expect("kept")["id"]
        .as_str()
        .expect("id")
        .to_string();
    // Carol read hers first: a read notification survives the delete.
    let carol_items = f.notifications(&f.carol).await;
    let carol_msg = carol_items
        .iter()
        .find(|n| n["messageId"] == json!(msg))
        .expect("carol's")["id"]
        .as_str()
        .expect("id")
        .to_string();
    f.mark_read(&f.carol, Some(vec![carol_msg]), None).await;

    exec_ok(
        &f.schema,
        f.bob.request(
            DELETE_MESSAGE,
            json!({ "input": {
                "serverId": f.server_id, "conversationId": f.general, "messageId": msg,
            } }),
        ),
    )
    .await;
    assert_eq!(
        f.reasons(&f.alice).await,
        vec![(keep.clone(), "ALL".to_string())]
    );
    assert_eq!(f.notifications(&f.alice).await[0]["id"], json!(alice_keep));
    assert_eq!(f.reasons(&f.carol).await.len(), 2, "the read row stays");
}

/// The three builtins run through `executeCommand` with no capability grant.
#[tokio::test]
async fn notification_commands_dispatch_to_the_service() {
    let f = setup().await;
    let result = f
        .execute(
            &f.alice,
            "notifications.set_level",
            json!({ "level": "none" }),
            Some(&f.general),
        )
        .await;
    assert_eq!(result["status"], "SUCCESS", "{result}");
    assert_eq!(result["payload"]["effective_level"], "none");
    let conv = f.conversation_level(&f.alice, &f.general).await;
    assert_eq!(conv["notificationLevel"], "NONE");

    let result = f
        .execute(
            &f.alice,
            "notifications.dnd",
            json!({ "mode": "until", "minutes": 30 }),
            None,
        )
        .await;
    assert_eq!(result["status"], "SUCCESS", "{result}");
    assert_eq!(result["payload"]["paused"], true);
    let settings = exec_ok(
        &f.schema,
        f.alice
            .request(SETTINGS, json!({ "serverId": f.server_id })),
    )
    .await["settings"]
        .clone();
    assert_eq!(settings["dndPaused"], true);
    assert!(settings["dndPausedUntil"].is_string());

    f.send(&f.alice, &f.general, "hi carol").await;
    assert_eq!(f.unread_count(&f.carol).await, 1);
    let result = f
        .execute(&f.carol, "notifications.mark_all_read", json!({}), None)
        .await;
    assert_eq!(result["status"], "SUCCESS", "{result}");
    assert_eq!(result["payload"]["marked"], 1);
    assert_eq!(f.unread_count(&f.carol).await, 0);
}

/// Workspace membership gates every query; a level for a conversation the caller cannot read
/// is refused without revealing it.
#[tokio::test]
async fn membership_gates() {
    let f = setup().await;
    let outsider_user = f.ctx.create_user("nt-outsider").await;
    let outsider = f.ctx.caller(&outsider_user).await;
    let code = exec_err_code(
        &f.schema,
        outsider.request(UNREAD_COUNT, json!({ "serverId": f.server_id })),
    )
    .await;
    assert_eq!(code, "FORBIDDEN");

    let secret = create_channel(&f.schema, &f.alice, &f.server_id, "secret", true).await;
    let mut vars = f.ids(&secret);
    vars["level"] = json!("ALL");
    let code = exec_err_code(&f.schema, f.carol.request(SET_LEVEL, vars)).await;
    assert_eq!(code, "NOT_FOUND");
}
