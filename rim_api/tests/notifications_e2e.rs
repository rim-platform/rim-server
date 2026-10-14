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

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::server_member::model::ServerMemberRole;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_infra_db::sea_orm_impl::notification::SeaOrmNotificationRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_infra_services::email::{EmailMessage, EmailSender};
use rim_infra_services::notification::digest::DigestRunReport;
use rim_infra_services::notification::{
    DefaultNotificationJobs, NotificationJobRepos, NotificationJobs,
};
use rim_infra_services::presence::PresenceManager;
use rim_services::auth::dto::RegistrationMode;
use rim_services::events::{NoopBroadcaster, Observers};

use support::graphql::{
    Caller, anonymous_request, assert_each_refused, assert_each_refused_as, exec_err_code, exec_ok,
};
use support::roles::create_channel;
use support::roots::{CONVERSATION_NOTIFICATION_ROOTS, Targets};
use support::{TestCtx, unknown_id};

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

const EDIT_MESSAGE: &str = r#"
mutation Edit($input: EditMessageInput!) { editMessage(input: $input) { id } }
"#;

const LEAVE_CONVERSATION: &str = r#"
mutation Leave($serverId: ID!, $conversationId: ID!) {
  leaveConversation(serverId: $serverId, conversationId: $conversationId)
}
"#;

const KICK: &str = r#"
mutation Kick($serverId: ID!, $userId: ID!) { kickMember(serverId: $serverId, userId: $userId) }
"#;

const MARK_AS_READ: &str = r#"
mutation MarkRead($input: MarkAsReadInput!) { markAsRead(input: $input) }
"#;

const MARK_ALL_AS_READ: &str = r#"
mutation MarkAllRead($workspaceId: ID!) { markAllAsRead(input: { workspaceId: $workspaceId }) }
"#;

const UNREAD_COUNTS: &str = r#"
query Unread($workspaceId: ID!) { unreadCounts(workspaceId: $workspaceId) { conversationId count } }
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

/// Collects the emails the digest job sends.
#[derive(Default)]
struct Outbox(Mutex<Vec<EmailMessage>>);

#[async_trait]
impl EmailSender for Outbox {
    async fn send(&self, message: EmailMessage) -> anyhow::Result<()> {
        self.0.lock().expect("lock").push(message);
        Ok(())
    }
}

impl Fixture {
    /// Make every stored notification two hours old, past the hourly digest interval.
    async fn age_notifications(&self) {
        self.ctx
            .conn
            .execute_unprepared(
                "UPDATE user_notification SET created_at = now() - interval '2 hours'",
            )
            .await
            .expect("age notifications");
    }

    /// Every stored notification of `who`, in any workspace, read or not.
    async fn stored_notifications(&self, who: &Caller) -> i64 {
        let row = self
            .ctx
            .conn
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM user_notification WHERE user_id = $1",
                [uuid::Uuid::from(who.user_id).into()],
            ))
            .await
            .expect("count")
            .expect("row");
        row.try_get("", "n").expect("n")
    }

    /// Run the digest job over the real repositories and return what it sent.
    async fn run_digest(&self) -> (DigestRunReport, Vec<EmailMessage>) {
        let db = &self.ctx.db;
        let outbox = Arc::new(Outbox::default());
        let observers = Observers::new(Arc::new(NoopBroadcaster));
        let jobs = DefaultNotificationJobs::new(
            NotificationJobRepos {
                notification_repo: Arc::new(SeaOrmNotificationRepository::new(db.clone())),
                message_repo: Arc::new(SeaOrmMessageRepository::new(db.clone())),
                conversation_repo: Arc::new(SeaOrmConversationRepository::new(db.clone())),
                user_repo: Arc::new(SeaOrmUserRepository::new(db.clone())),
                server_member_repo: Arc::new(SeaOrmServerMemberRepository::new(db.clone())),
                conversation_member_repo: Arc::new(SeaOrmConversationMemberRepository::new(
                    db.clone(),
                )),
            },
            Arc::clone(&outbox) as Arc<dyn EmailSender>,
            Arc::new(PresenceManager::new(Arc::new(observers))),
            None,
        );
        let report = jobs.run_digest(chrono::Utc::now()).await.expect("digest");
        let sent = outbox.0.lock().expect("lock").clone();
        (report, sent)
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

    // markAllAsRead reads every joined conversation and the rest of the notifications.
    let workspace = json!({ "workspaceId": f.server_id });
    let unread = || async {
        let data = exec_ok(&f.schema, f.alice.request(UNREAD_COUNTS, workspace.clone())).await;
        let counts = data["unreadCounts"].as_array().expect("counts").clone();
        counts
            .into_iter()
            .filter(|c| c["count"] != 0)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        unread().await,
        [json!({ "conversationId": other, "count": 1 })]
    );
    let data = exec_ok(
        &f.schema,
        f.alice.request(MARK_ALL_AS_READ, workspace.clone()),
    )
    .await;
    assert_eq!(data["markAllAsRead"], true);
    assert_eq!(unread().await, Vec::<Value>::new());
    assert_eq!(f.unread_count(&f.alice).await, 0);
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

    // Once removed, the channel's notifications leave the inbox; the others stay.
    let in_general = f.send(&f.alice, &f.general, "for everyone").await;
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
    assert_eq!(
        f.reasons(&f.bob).await,
        vec![(in_general, "ALL".to_string())]
    );
    assert_eq!(f.unread_count(&f.bob).await, 1);
}

#[tokio::test]
async fn leaving_a_conversation_drops_only_its_notifications() {
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
    let in_general = f.send(&f.alice, &f.general, "for everyone").await;
    f.send(&f.alice, &secret, "for bob").await;
    assert_eq!(f.unread_count(&f.bob).await, 2);

    exec_ok(&f.schema, f.bob.request(LEAVE_CONVERSATION, f.ids(&secret))).await;
    assert_eq!(
        f.reasons(&f.bob).await,
        vec![(in_general, "ALL".to_string())]
    );
    assert_eq!(f.unread_count(&f.bob).await, 1);
}

/// A kicked member's pending notifications go with them: an edit made after the kick never
/// reaches them by email, while other members still get their digest.
#[tokio::test]
async fn kicked_member_gets_no_digest_of_a_later_edit() {
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
    for who in [&f.bob, &f.carol] {
        f.update_settings(who, settings_input("ALL", &[], "HOURLY"))
            .await;
    }
    let before = f.send(&f.alice, &secret, "before removal").await;
    f.send(&f.alice, &f.general, "general news").await;
    f.age_notifications().await;

    exec_ok(
        &f.schema,
        f.alice.request(
            KICK,
            json!({ "serverId": f.server_id, "userId": f.bob.user_id.to_string() }),
        ),
    )
    .await;
    assert_eq!(f.stored_notifications(&f.bob).await, 0);
    exec_ok(
        &f.schema,
        f.alice.request(
            EDIT_MESSAGE,
            json!({ "input": {
                "serverId": f.server_id, "conversationId": secret, "messageId": before,
                "text": "SECRET ADDED AFTER REMOVAL",
            } }),
        ),
    )
    .await;

    let (report, sent) = f.run_digest().await;
    assert_eq!((report.candidates, report.sent), (1, 1));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, "nt-carol@test.example.com");
    assert!(sent[0].body_text.contains("general news"));
    assert!(sent.iter().all(|m| !m.body_text.contains("SECRET")));
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

/// Roots over the caller's notifications in workspace `$s`.
const NOTIFICATION_ROOTS: &[&str] = &[
    "query($s: ID!) { myNotificationSettings(serverId: $s) { defaultLevel } }",
    "query($s: ID!) { myNotifications(serverId: $s) { items { id } } }",
    "query($s: ID!) { myUnreadNotificationCount(serverId: $s) }",
    "query($s: ID!) { mutedConversations(serverId: $s) { conversationId } }",
    "mutation($s: ID!) { markNotificationsRead(serverId: $s, \
     allBefore: \"2026-01-01T00:00:00Z\") }",
    "mutation($s: ID!) { setDoNotDisturb(serverId: $s, paused: true) { dndPaused } }",
    "mutation($s: ID!) { updateNotificationSettings(serverId: $s, input: { defaultLevel: NONE, \
     keywords: [], digest: OFF }) { defaultLevel } }",
];

/// Every notification root refuses a caller without a session and one outside the workspace,
/// and a guest nobody added to a channel sets no rule for it. None of them changes anything.
#[tokio::test]
async fn every_notification_root_refuses_callers_outside_the_workspace() {
    let f = setup().await;
    let t = Targets::conversation(&f.server_id, &f.general, &f.bob);
    let calls = t.calls(&[NOTIFICATION_ROOTS, CONVERSATION_NOTIFICATION_ROOTS]);
    assert_each_refused(&f.schema, &calls, "UNAUTHENTICATED", anonymous_request).await;
    let olive = f.ctx.outsider("nt-olive").await;
    assert_each_refused_as(&f.schema, &olive, &calls, "FORBIDDEN").await;
    let gina = f
        .ctx
        .newcomer(&f.server_id, "nt-gina", ServerMemberRole::Guest)
        .await;
    let rules = t.calls(&[CONVERSATION_NOTIFICATION_ROOTS]);
    assert_each_refused_as(&f.schema, &gina, &rules, "NOT_FOUND").await;

    let conv = f.conversation_level(&f.alice, &f.general).await;
    assert_eq!(
        (&conv["isMuted"], &conv["notificationLevel"]),
        (&json!(false), &Value::Null)
    );
}

/// An id that names nothing is `NOT_FOUND`; a malformed one is `VALIDATION`.
#[tokio::test]
async fn every_notification_root_answers_unknown_and_malformed_ids() {
    let f = setup().await;
    let nowhere = Targets::conversation(&f.server_id, &unknown_id(), &f.bob);
    let calls = nowhere.calls(&[CONVERSATION_NOTIFICATION_ROOTS]);
    assert_each_refused_as(&f.schema, &f.alice, &calls, "NOT_FOUND").await;

    let mut malformed = Targets::conversation(&f.server_id, "not-an-id", &f.bob);
    let mut calls = malformed.calls(&[CONVERSATION_NOTIFICATION_ROOTS]);
    malformed.server = "not-an-id".to_string();
    calls.extend(malformed.calls(&[NOTIFICATION_ROOTS]));
    let ids = json!({ "serverId": f.server_id, "ids": ["not-an-id"] });
    calls.push((MARK_NOTIFICATIONS_READ, ids));
    assert_each_refused_as(&f.schema, &f.alice, &calls, "VALIDATION").await;
}

const MUTED: &str =
    "query($s: ID!) { muted: mutedConversations(serverId: $s) { conversationId mutedUntil } }";

/// `mutedConversations` lists the caller's mutes in the workspace asked about only, and a mute
/// names a conversation of the workspace it is set in.
#[tokio::test]
async fn muted_conversations_stay_in_their_workspace() {
    let f = setup().await;
    let other = f.ctx.create_server("nt-other").await.id.to_string();
    f.ctx.join(&other, &f.alice, ServerMemberRole::Owner).await;
    let elsewhere = create_channel(&f.schema, &f.alice, &other, "elsewhere", false).await;
    let mute = |server_id: &str, conversation_id: &str| {
        let input = json!({ "serverId": server_id, "conversationId": conversation_id });
        f.alice.request(MUTE, json!({ "input": input }))
    };
    exec_ok(&f.schema, mute(&f.server_id, &f.general)).await;
    exec_ok(&f.schema, mute(&other, &elsewhere)).await;
    let code = exec_err_code(&f.schema, mute(&f.server_id, &elsewhere)).await;
    assert_eq!(code, "NOT_FOUND", "a conversation of another workspace");

    for (server_id, muted) in [(&f.server_id, &f.general), (&other, &elsewhere)] {
        let data = exec_ok(&f.schema, f.alice.request(MUTED, json!({ "s": server_id }))).await;
        assert_eq!(
            data["muted"],
            json!([{ "conversationId": muted, "mutedUntil": null }])
        );
    }
}
