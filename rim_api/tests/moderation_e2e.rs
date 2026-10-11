//! Through-the-stack e2e tests for roles and moderation.
//!
//! Every assertion goes through GraphQL: resolver → `ModerationService` / `MessageService` /
//! `ConversationService` / `AuthService` / `CommandService` → SeaORM repositories → Postgres.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::server_member::model::ServerMemberRole;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{Caller, anonymous_request, exec_err_code, exec_ok, merged};
use support::roles::create_channel;

const CREATE_CHANNEL: &str = r#"
mutation Create($input: CreateConversationInput!) {
  conversation: createConversation(input: $input) { id slowModeSeconds }
}
"#;

const ADD_MEMBERS: &str = r#"
mutation Add($serverId: ID!, $conversationId: ID!, $userIds: [ID!]!) {
  conversation: addConversationMembers(serverId: $serverId, conversationId: $conversationId,
                                       userIds: $userIds) { id }
}
"#;

const JOIN_CHANNEL: &str = r#"
mutation Join($serverId: ID!, $conversationId: ID!) {
  conversation: joinChannel(serverId: $serverId, conversationId: $conversationId) { id }
}
"#;

const CONVERSATION_QUERY: &str = r#"
query Conversation($serverId: ID!, $conversationId: ID!) {
  conversation(serverId: $serverId, conversationId: $conversationId) { id slowModeSeconds }
}
"#;

const CHANNELS_QUERY: &str = r#"
query Channels($serverId: ID!) {
  channels(serverId: $serverId, dir: AFTER, limit: 50, filter: { joinedOnly: false }) {
    items { id }
  }
}
"#;

const SEND_MESSAGE: &str = r#"
mutation Send($input: SendMessageInput!) { msg: sendMessage(input: $input) { id } }
"#;

const DELETE_MESSAGE: &str = r#"
mutation Delete($input: DeleteMessageInput!) { deleteMessage(input: $input) }
"#;

const MESSAGES_QUERY: &str = r#"
query Messages($serverId: ID!, $conversationId: ID!) {
  page: messages(serverId: $serverId, conversationId: $conversationId, dir: AFTER, limit: 50,
                 includeDeleted: false) { items { id } }
}
"#;

const SERVER_MEMBERS: &str = r#"
query Members($serverId: ID!) {
  serverMembers(serverId: $serverId) { userId memberRole timeoutUntil }
}
"#;

const CREATE_INVITE: &str = r#"
mutation CreateInvite($input: CreateInviteInput!) {
  invite: createInvite(input: $input) { inviteToken }
}
"#;

const JOIN_BY_INVITE: &str = r#"
mutation JoinByInvite($input: JoinServerByInviteInput!) {
  joinServerByInvite(input: $input) { serverId }
}
"#;

const SET_MEMBER_ROLE: &str = r#"
mutation SetRole($serverId: ID!, $userId: ID!, $role: ServerMemberRoleGql!) {
  member: setMemberRole(serverId: $serverId, userId: $userId, role: $role) { userId memberRole }
}
"#;

const KICK: &str = r#"
mutation Kick($serverId: ID!, $userId: ID!) {
  kickMember(serverId: $serverId, userId: $userId, reason: "be nice")
}
"#;

const BAN: &str = r#"
mutation Ban($serverId: ID!, $userId: ID!) {
  ban: banMember(serverId: $serverId, userId: $userId, reason: "spam") {
    userId reason expiresAt user { id }
  }
}
"#;

const UNBAN: &str = r#"
mutation Unban($serverId: ID!, $userId: ID!) { unbanMember(serverId: $serverId, userId: $userId) }
"#;

const BANS: &str = r#"
query Bans($serverId: ID!) { serverBans(serverId: $serverId) { userId reason } }
"#;

const TIMEOUT: &str = r#"
mutation Timeout($serverId: ID!, $userId: ID!, $until: DateTime!) {
  timeout: timeoutMember(serverId: $serverId, userId: $userId, until: $until) { userId until }
}
"#;

const CLEAR_TIMEOUT: &str = r#"
mutation Clear($serverId: ID!, $userId: ID!) { clearTimeout(serverId: $serverId, userId: $userId) }
"#;

const SET_SLOW_MODE: &str = r#"
mutation Slow($serverId: ID!, $conversationId: ID!, $seconds: Int!) {
  conversation: setSlowMode(serverId: $serverId, conversationId: $conversationId,
                            seconds: $seconds) { id slowModeSeconds }
}
"#;

const REPORT: &str = r#"
mutation Report($serverId: ID!, $conversationId: ID!, $messageId: ID!, $reason: String!) {
  report: reportMessage(serverId: $serverId, conversationId: $conversationId,
                        messageId: $messageId, reason: $reason) { id status reason }
}
"#;

const REPORTS: &str = r#"
query Reports($serverId: ID!, $status: MessageReportStatus) {
  page: messageReports(serverId: $serverId, status: $status, dir: AFTER, limit: 20) {
    items { id messageId reporterUserId reason status messageReadable messageText messageAuthorId }
    pageInfo { hasNext hasPrev }
  }
}
"#;

const RESOLVE: &str = r#"
mutation Resolve($serverId: ID!, $reportId: ID!, $outcome: ReportOutcome!) {
  report: resolveReport(serverId: $serverId, reportId: $reportId, outcome: $outcome) {
    id status resolvedBy
  }
}
"#;

const EXECUTE: &str = r#"
mutation Exec($serverId: ID!, $args: JSON!, $commandId: String! = "chat.post_message") {
  executeCommand(input: {
    commandId: $commandId
    args: $args
    commandContext: { workspaceId: $serverId }
  }) { status message }
}
"#;

const AUDIT_LOG: &str = r#"
query Audit($filter: AuditLogFilterInput, $pagination: AuditLogPaginationInput) {
  listAuditLog(filter: $filter, pagination: $pagination) {
    items {
      id actorUserId serverId commandId initiator argsRedacted sensitivity dangerous
      confirmationKind typedVerb outcome errorKind capabilityCheckOutcome confirmationOutcome
      createdAt
    }
    nextCursor
    pageInfo { hasNextPage hasPrevPage }
  }
}
"#;

struct Fixture {
    ctx: TestCtx,
    schema: AppSchema,
    server: rim_core::server::model::Server,
    server_id: String,
    /// Owner; created the public channel.
    alice: Caller,
    mona: Caller,
    bob: Caller,
    carol: Caller,
    gina: Caller,
    channel_id: String,
}

/// alice (Owner), mona (Moderator), bob and carol (Member), gina (Guest) and a public channel
/// alice created.
async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let server = ctx.create_server("moderation-e2e").await;
    let alice_user = ctx.create_user("mod-alice").await;
    let mona_user = ctx.create_user("mod-mona").await;
    let bob_user = ctx.create_user("mod-bob").await;
    let carol_user = ctx.create_user("mod-carol").await;
    let gina_user = ctx.create_user("mod-gina").await;
    ctx.add_owner(&server, &alice_user).await;
    ctx.add_member(&server, &mona_user, ServerMemberRole::Moderator)
        .await;
    ctx.add_member(&server, &bob_user, ServerMemberRole::Member)
        .await;
    ctx.add_member(&server, &carol_user, ServerMemberRole::Member)
        .await;
    ctx.add_member(&server, &gina_user, ServerMemberRole::Guest)
        .await;

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let alice = ctx.caller(&alice_user).await;
    let server_id = server.id.to_string();
    let channel_id = create_channel(&schema, &alice, &server_id, "general", false).await;

    Fixture {
        mona: ctx.caller(&mona_user).await,
        bob: ctx.caller(&bob_user).await,
        carol: ctx.caller(&carol_user).await,
        gina: ctx.caller(&gina_user).await,
        alice,
        server,
        server_id,
        channel_id,
        schema,
        ctx,
    }
}

impl Fixture {
    async fn send(&self, who: &Caller, conversation_id: &str, text: &str) -> String {
        let data = exec_ok(
            &self.schema,
            who.request(
                SEND_MESSAGE,
                json!({ "input": {
                    "serverId": self.server_id,
                    "conversationId": conversation_id,
                    "text": text,
                } }),
            ),
        )
        .await;
        data["msg"]["id"].as_str().expect("message id").to_string()
    }

    fn send_request(&self, who: &Caller, text: &str) -> async_graphql::Request {
        who.request(
            SEND_MESSAGE,
            json!({ "input": {
                "serverId": self.server_id,
                "conversationId": self.channel_id,
                "text": text,
            } }),
        )
    }

    fn member_vars(&self, user: &Caller) -> Value {
        json!({ "serverId": self.server_id, "userId": user.user_id.to_string() })
    }

    async fn invite_token(&self) -> String {
        let data = exec_ok(
            &self.schema,
            self.alice.request(
                CREATE_INVITE,
                json!({ "input": { "serverId": self.server_id, "maxUses": 1 } }),
            ),
        )
        .await;
        data["invite"]["inviteToken"]
            .as_str()
            .expect("token")
            .to_string()
    }

    async fn member_ids(&self, who: &Caller) -> Vec<String> {
        let data = exec_ok(
            &self.schema,
            who.request(SERVER_MEMBERS, json!({ "serverId": self.server_id })),
        )
        .await;
        let mut ids: Vec<String> = data["serverMembers"]
            .as_array()
            .expect("members")
            .iter()
            .map(|m| m["userId"].as_str().expect("id").to_string())
            .collect();
        ids.sort();
        ids
    }

    /// The audit log page alice reads with `filter` and `pagination`.
    async fn audit_page(&self, filter: Value, pagination: Value) -> Value {
        let vars = json!({ "filter": filter, "pagination": pagination });
        exec_ok(&self.schema, self.alice.request(AUDIT_LOG, vars)).await["listAuditLog"].clone()
    }

    async fn audit_rows(&self, command_id: &str) -> Vec<entity::command_audit_log::Model> {
        let server_uuid: uuid::Uuid = self.server.id.into();
        entity::command_audit_log::Entity::find()
            .filter(entity::command_audit_log::Column::ServerId.eq(server_uuid))
            .filter(entity::command_audit_log::Column::CommandId.eq(command_id))
            .all(&self.ctx.conn)
            .await
            .expect("audit query")
    }
}

/// Seed the builtin capabilities into the registry, as `rim_server` does at boot, so
/// `executeCommand` passes the capability gate and reaches the timeout gate.
fn error_extension(res: &async_graphql::Response, key: &str) -> Option<async_graphql::Value> {
    res.errors
        .first()
        .and_then(|e| e.extensions.as_ref())
        .and_then(|ext| ext.get(key).cloned())
}

#[tokio::test]
async fn kicked_member_may_rejoin_but_banned_member_may_not() {
    let f = setup().await;

    // Nobody kicks the owner; a moderator cannot kick a moderator either.
    let code = exec_err_code(&f.schema, f.mona.request(KICK, f.member_vars(&f.alice))).await;
    assert_eq!(code, "FORBIDDEN");
    let code = exec_err_code(&f.schema, f.bob.request(KICK, f.member_vars(&f.carol))).await;
    assert_eq!(code, "FORBIDDEN", "members cannot kick");

    let data = exec_ok(&f.schema, f.mona.request(KICK, f.member_vars(&f.bob))).await;
    assert_eq!(data["kickMember"], json!(true));
    assert!(
        !f.member_ids(&f.alice)
            .await
            .contains(&f.bob.user_id.to_string())
    );
    // The kicked user lost access to the workspace.
    let code = exec_err_code(&f.schema, f.send_request(&f.bob, "still here?")).await;
    assert_eq!(code, "FORBIDDEN");

    // A kick does not block re-joining.
    let token = f.invite_token().await;
    exec_ok(
        &f.schema,
        f.bob
            .request(JOIN_BY_INVITE, json!({ "input": { "inviteToken": token } })),
    )
    .await;
    assert!(
        f.member_ids(&f.alice)
            .await
            .contains(&f.bob.user_id.to_string())
    );

    // A ban removes and blocks.
    let data = exec_ok(&f.schema, f.mona.request(BAN, f.member_vars(&f.bob))).await;
    assert_eq!(data["ban"]["reason"], json!("spam"));
    assert_eq!(data["ban"]["expiresAt"], Value::Null);
    assert_eq!(data["ban"]["user"]["id"], json!(f.bob.user_id.to_string()));
    let bans = exec_ok(
        &f.schema,
        f.mona.request(BANS, json!({ "serverId": f.server_id })),
    )
    .await;
    assert_eq!(
        bans["serverBans"],
        json!([{ "userId": f.bob.user_id.to_string(), "reason": "spam" }])
    );
    let code = exec_err_code(
        &f.schema,
        f.carol.request(BANS, json!({ "serverId": f.server_id })),
    )
    .await;
    assert_eq!(code, "FORBIDDEN", "only moderators list bans");

    let token = f.invite_token().await;
    let code = exec_err_code(
        &f.schema,
        f.bob.request(
            JOIN_BY_INVITE,
            json!({ "input": { "inviteToken": token.clone() } }),
        ),
    )
    .await;
    assert_eq!(
        code, "FORBIDDEN",
        "a banned user cannot accept a new invite"
    );

    let data = exec_ok(&f.schema, f.mona.request(UNBAN, f.member_vars(&f.bob))).await;
    assert_eq!(data["unbanMember"], json!(true));
    exec_ok(
        &f.schema,
        f.bob
            .request(JOIN_BY_INVITE, json!({ "input": { "inviteToken": token } })),
    )
    .await;

    assert_eq!(f.audit_rows("admin.kick").await.len(), 1);
    assert_eq!(f.audit_rows("admin.ban").await.len(), 1);
    assert_eq!(f.audit_rows("admin.unban").await.len(), 1);
    let ban_row = &f.audit_rows("admin.ban").await[0];
    assert_eq!(ban_row.actor_user_id, uuid::Uuid::from(f.mona.user_id));
    assert_eq!(ban_row.initiator, "moderation");
}

#[tokio::test]
async fn timed_out_member_cannot_send_or_run_mutating_commands() {
    let f = setup().await;
    f.ctx.seed_capabilities().await;
    let until = chrono::Utc::now() + chrono::Duration::minutes(10);
    let vars = json!({
        "serverId": f.server_id,
        "userId": f.carol.user_id.to_string(),
        "until": until.to_rfc3339(),
    });
    let code = exec_err_code(&f.schema, f.bob.request(TIMEOUT, vars.clone())).await;
    assert_eq!(code, "FORBIDDEN", "members cannot time people out");
    exec_ok(&f.schema, f.mona.request(TIMEOUT, vars)).await;

    let res = f.schema.execute(f.send_request(&f.carol, "hi")).await;
    assert_eq!(
        error_extension(&res, "code"),
        Some(async_graphql::Value::from("TIMED_OUT"))
    );
    assert!(error_extension(&res, "until").is_some());

    let exec_vars = json!({
        "serverId": f.server_id,
        "args": { "server_id": f.server_id, "conversation_id": f.channel_id, "text": "via palette" },
    });
    let code = exec_err_code(&f.schema, f.carol.request(EXECUTE, exec_vars.clone())).await;
    assert_eq!(code, "TIMED_OUT");

    // Reads continue, and the directory shows the timeout.
    exec_ok(
        &f.schema,
        f.carol.request(
            MESSAGES_QUERY,
            json!({ "serverId": f.server_id, "conversationId": f.channel_id }),
        ),
    )
    .await;
    let members = exec_ok(
        &f.schema,
        f.alice
            .request(SERVER_MEMBERS, json!({ "serverId": f.server_id })),
    )
    .await;
    let carol = members["serverMembers"]
        .as_array()
        .expect("members")
        .iter()
        .find(|m| m["userId"] == json!(f.carol.user_id.to_string()))
        .expect("carol")
        .clone();
    assert!(carol["timeoutUntil"].is_string());
    assert_eq!(carol["memberRole"], json!("MEMBER"));

    let data = exec_ok(
        &f.schema,
        f.mona.request(CLEAR_TIMEOUT, f.member_vars(&f.carol)),
    )
    .await;
    assert_eq!(data["clearTimeout"], json!(true));
    f.send(&f.carol, &f.channel_id, "back").await;
    let ok = exec_ok(&f.schema, f.carol.request(EXECUTE, exec_vars)).await;
    assert_eq!(ok["executeCommand"]["status"], json!("SUCCESS"));
    assert_eq!(f.audit_rows("admin.timeout").await.len(), 1);
    assert_eq!(f.audit_rows("admin.clear_timeout").await.len(), 1);
}

#[tokio::test]
async fn slow_mode_rate_limits_members_but_not_moderators() {
    let f = setup().await;
    let vars = |seconds: i64| json!({ "serverId": f.server_id, "conversationId": f.channel_id, "seconds": seconds });
    let code = exec_err_code(&f.schema, f.bob.request(SET_SLOW_MODE, vars(60))).await;
    assert_eq!(code, "FORBIDDEN");
    let code = exec_err_code(&f.schema, f.mona.request(SET_SLOW_MODE, vars(21_601))).await;
    assert_eq!(code, "VALIDATION");
    let data = exec_ok(&f.schema, f.mona.request(SET_SLOW_MODE, vars(60))).await;
    assert_eq!(data["conversation"]["slowModeSeconds"], json!(60));
    let read = exec_ok(
        &f.schema,
        f.bob.request(
            CONVERSATION_QUERY,
            json!({ "serverId": f.server_id, "conversationId": f.channel_id }),
        ),
    )
    .await;
    assert_eq!(read["conversation"]["slowModeSeconds"], json!(60));

    f.send(&f.bob, &f.channel_id, "first").await;
    let res = f.schema.execute(f.send_request(&f.bob, "second")).await;
    assert_eq!(
        error_extension(&res, "code"),
        Some(async_graphql::Value::from("RATE_LIMITED"))
    );
    match error_extension(&res, "retryAfterSeconds") {
        Some(async_graphql::Value::Number(n)) => {
            let secs = n.as_i64().expect("integer");
            assert!((1..=60).contains(&secs), "{secs}");
        }
        other => panic!("missing retryAfterSeconds: {other:?}"),
    }

    // Moderators and the owner are exempt.
    f.send(&f.mona, &f.channel_id, "mod one").await;
    f.send(&f.mona, &f.channel_id, "mod two").await;
    f.send(&f.alice, &f.channel_id, "owner one").await;
    f.send(&f.alice, &f.channel_id, "owner two").await;

    // Turning it off lets bob post again.
    exec_ok(&f.schema, f.mona.request(SET_SLOW_MODE, vars(0))).await;
    f.send(&f.bob, &f.channel_id, "second, finally").await;
    assert_eq!(f.audit_rows("chat.set_slow_mode").await.len(), 2);
}

#[tokio::test]
async fn guest_cannot_create_join_invite_or_browse() {
    let f = setup().await;

    let code = exec_err_code(
        &f.schema,
        f.gina.request(
            CREATE_CHANNEL,
            json!({ "input": { "serverId": f.server_id, "isChannel": true, "title": "guestroom" } }),
        ),
    )
    .await;
    assert_eq!(code, "FORBIDDEN", "guests cannot create channels");
    let code = exec_err_code(
        &f.schema,
        f.gina.request(
            JOIN_CHANNEL,
            json!({ "serverId": f.server_id, "conversationId": f.channel_id }),
        ),
    )
    .await;
    assert_eq!(code, "FORBIDDEN", "guests cannot join channels");
    let code = exec_err_code(
        &f.schema,
        f.gina.request(
            CREATE_INVITE,
            json!({ "input": { "serverId": f.server_id, "maxUses": 1 } }),
        ),
    )
    .await;
    assert_eq!(code, "FORBIDDEN", "guests cannot invite");
    let code = exec_err_code(
        &f.schema,
        f.gina.request(
            CONVERSATION_QUERY,
            json!({ "serverId": f.server_id, "conversationId": f.channel_id }),
        ),
    )
    .await;
    assert_eq!(code, "NOT_FOUND", "an unjoined public channel stays hidden");
    let channels = exec_ok(
        &f.schema,
        f.gina
            .request(CHANNELS_QUERY, json!({ "serverId": f.server_id })),
    )
    .await;
    assert_eq!(channels["channels"]["items"], json!([]), "no browsing");
    assert_eq!(
        f.member_ids(&f.gina).await,
        vec![f.gina.user_id.to_string()],
        "a guest sharing no conversation sees only themselves"
    );

    // Added by a member, the guest reads and writes there like anyone, sees the people in it,
    // and still cannot invite.
    exec_ok(
        &f.schema,
        f.alice.request(
            ADD_MEMBERS,
            json!({
                "serverId": f.server_id,
                "conversationId": f.channel_id,
                "userIds": [f.gina.user_id.to_string()],
            }),
        ),
    )
    .await;
    f.send(&f.gina, &f.channel_id, "hello from a guest").await;
    let channels = exec_ok(
        &f.schema,
        f.gina
            .request(CHANNELS_QUERY, json!({ "serverId": f.server_id })),
    )
    .await;
    assert_eq!(
        channels["channels"]["items"],
        json!([{ "id": f.channel_id }])
    );
    let mut expected = vec![f.alice.user_id.to_string(), f.gina.user_id.to_string()];
    expected.sort();
    assert_eq!(f.member_ids(&f.gina).await, expected);
    let code = exec_err_code(
        &f.schema,
        f.gina.request(
            ADD_MEMBERS,
            json!({
                "serverId": f.server_id,
                "conversationId": f.channel_id,
                "userIds": [f.bob.user_id.to_string()],
            }),
        ),
    )
    .await;
    assert_eq!(code, "FORBIDDEN");
}

#[tokio::test]
async fn moderator_deletes_others_messages_with_an_audit_row() {
    let f = setup().await;
    let message_id = f.send(&f.bob, &f.channel_id, "rude words").await;
    let delete_vars = json!({ "input": {
        "serverId": f.server_id,
        "conversationId": f.channel_id,
        "messageId": message_id,
    } });

    let code = exec_err_code(
        &f.schema,
        f.carol.request(DELETE_MESSAGE, delete_vars.clone()),
    )
    .await;
    assert_eq!(code, "FORBIDDEN", "members delete only their own messages");
    let data = exec_ok(&f.schema, f.mona.request(DELETE_MESSAGE, delete_vars)).await;
    assert_eq!(data["deleteMessage"], json!(true));

    let rows = f.audit_rows("admin.delete_message").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_user_id, uuid::Uuid::from(f.mona.user_id));
    assert_eq!(
        rows[0].args_redacted["author_id"],
        json!(f.bob.user_id.to_string())
    );
    assert_eq!(rows[0].args_redacted["message_id"], json!(message_id));
    assert!(
        !rows[0].args_redacted.to_string().contains("rude"),
        "message text is redacted"
    );
}

#[tokio::test]
async fn report_list_and_resolve() {
    let f = setup().await;
    let message_id = f.send(&f.bob, &f.channel_id, "buy cheap watches").await;
    let report_vars = |reason: &str| {
        json!({
            "serverId": f.server_id,
            "conversationId": f.channel_id,
            "messageId": message_id,
            "reason": reason,
        })
    };

    let data = exec_ok(&f.schema, f.carol.request(REPORT, report_vars("spam"))).await;
    assert_eq!(data["report"]["status"], json!("OPEN"));
    let report_id = data["report"]["id"].as_str().expect("id").to_string();
    let code = exec_err_code(&f.schema, f.carol.request(REPORT, report_vars("spam!"))).await;
    assert_eq!(code, "CONFLICT", "one report per message and reporter");

    // A private channel the moderator is not in: listed, but without the message body.
    let private_id = create_channel(&f.schema, &f.bob, &f.server_id, "secret", true).await;
    exec_ok(
        &f.schema,
        f.bob.request(
            ADD_MEMBERS,
            json!({ "serverId": f.server_id, "conversationId": private_id,
                    "userIds": [f.carol.user_id.to_string()] }),
        ),
    )
    .await;
    let secret_id = f.send(&f.bob, &private_id, "private insult").await;
    exec_ok(
        &f.schema,
        f.carol.request(
            REPORT,
            json!({ "serverId": f.server_id, "conversationId": private_id,
                    "messageId": secret_id, "reason": "harassment" }),
        ),
    )
    .await;

    let code = exec_err_code(
        &f.schema,
        f.bob.request(
            REPORTS,
            json!({ "serverId": f.server_id, "status": "OPEN" }),
        ),
    )
    .await;
    assert_eq!(code, "FORBIDDEN", "only moderators list reports");

    let listed = exec_ok(
        &f.schema,
        f.mona.request(
            REPORTS,
            json!({ "serverId": f.server_id, "status": "OPEN" }),
        ),
    )
    .await;
    let items = listed["page"]["items"].as_array().expect("items").clone();
    assert_eq!(items.len(), 2);
    // Newest first: the second report (on the private channel's message) comes first.
    let (newest, older) = (&items[0], &items[1]);
    assert_eq!(newest["messageId"], json!(secret_id));
    assert_eq!(newest["messageReadable"], json!(false));
    assert_eq!(newest["messageText"], Value::Null);
    assert_eq!(newest["messageAuthorId"], Value::Null);
    assert_eq!(newest["reason"], json!("harassment"));
    assert_eq!(older["messageId"], json!(message_id));
    assert_eq!(older["messageReadable"], json!(true));
    assert_eq!(older["messageText"], json!("buy cheap watches"));
    assert_eq!(older["messageAuthorId"], json!(f.bob.user_id.to_string()));
    assert_eq!(older["reporterUserId"], json!(f.carol.user_id.to_string()));

    let resolve_vars =
        json!({ "serverId": f.server_id, "reportId": report_id, "outcome": "DISMISSED" });
    let resolved = exec_ok(&f.schema, f.mona.request(RESOLVE, resolve_vars.clone())).await;
    assert_eq!(resolved["report"]["status"], json!("DISMISSED"));
    assert_eq!(
        resolved["report"]["resolvedBy"],
        json!(f.mona.user_id.to_string())
    );
    let code = exec_err_code(&f.schema, f.mona.request(RESOLVE, resolve_vars)).await;
    assert_eq!(code, "CONFLICT", "a closed report cannot be resolved again");

    let open = exec_ok(
        &f.schema,
        f.mona.request(
            REPORTS,
            json!({ "serverId": f.server_id, "status": "OPEN" }),
        ),
    )
    .await;
    assert_eq!(open["page"]["items"].as_array().expect("items").len(), 1);
    assert_eq!(f.audit_rows("chat.report_message").await.len(), 2);
    assert_eq!(f.audit_rows("admin.resolve_report").await.len(), 1);
}

#[tokio::test]
async fn set_member_role_matrix() {
    let f = setup().await;
    let set = |who: &Caller, target: &Caller, role: &str| {
        who.request(
            SET_MEMBER_ROLE,
            json!({ "serverId": f.server_id, "userId": target.user_id.to_string(), "role": role }),
        )
    };

    let data = exec_ok(&f.schema, set(&f.alice, &f.bob, "MODERATOR")).await;
    assert_eq!(data["member"]["memberRole"], json!("MODERATOR"));
    let data = exec_ok(&f.schema, set(&f.mona, &f.carol, "GUEST")).await;
    assert_eq!(data["member"]["memberRole"], json!("GUEST"));
    let data = exec_ok(&f.schema, set(&f.mona, &f.gina, "MEMBER")).await;
    assert_eq!(data["member"]["memberRole"], json!("MEMBER"));

    for (who, target, role, expected) in [
        (&f.mona, &f.gina, "MODERATOR", "FORBIDDEN"),
        (&f.mona, &f.bob, "MEMBER", "FORBIDDEN"),
        (&f.mona, &f.alice, "MEMBER", "FORBIDDEN"),
        (&f.carol, &f.gina, "GUEST", "FORBIDDEN"),
        (&f.alice, &f.alice, "MEMBER", "VALIDATION"),
        (&f.alice, &f.bob, "OWNER", "VALIDATION"),
    ] {
        let code = exec_err_code(&f.schema, set(who, target, role)).await;
        assert_eq!(
            code, expected,
            "{:?} -> {:?} {role}",
            who.user_id, target.user_id
        );
    }

    let members = exec_ok(
        &f.schema,
        f.alice
            .request(SERVER_MEMBERS, json!({ "serverId": f.server_id })),
    )
    .await;
    let role_of = |c: &Caller| {
        members["serverMembers"]
            .as_array()
            .expect("members")
            .iter()
            .find(|m| m["userId"] == json!(c.user_id.to_string()))
            .map(|m| m["memberRole"].clone())
    };
    assert_eq!(role_of(&f.alice), Some(json!("OWNER")));
    assert_eq!(role_of(&f.bob), Some(json!("MODERATOR")));
    assert_eq!(role_of(&f.carol), Some(json!("GUEST")));
    assert_eq!(role_of(&f.gina), Some(json!("MEMBER")));
    assert_eq!(f.audit_rows("admin.set_member_role").await.len(), 3);
}

/// Owners and moderators read the workspace audit log, with or without another user's id in
/// the filter; members, guests and outsiders are refused both and members and guests still read
/// their own rows. Without a session the log answers `UNAUTHENTICATED`.
#[tokio::test]
async fn audit_log_workspace_reads_need_owner_or_moderator() {
    let f = setup().await;
    f.ctx.seed_capabilities().await;
    let exec_vars = json!({
        "serverId": f.server_id,
        "args": { "server_id": f.server_id, "conversation_id": f.channel_id, "text": "logged" },
    });
    let ok = exec_ok(&f.schema, f.bob.request(EXECUTE, exec_vars)).await;
    assert_eq!(ok["executeCommand"]["status"], json!("SUCCESS"));

    let bob_id = f.bob.user_id.to_string();
    let workspace = json!({ "filter": { "serverId": f.server_id } });
    let cross_user = json!({ "filter": { "serverId": f.server_id, "actorUserId": bob_id } });
    let bob_rows = |data: &Value| {
        data["listAuditLog"]["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter(|row| row["actorUserId"] == json!(bob_id))
            .count()
    };

    for (who, name) in [(&f.alice, "owner"), (&f.mona, "moderator")] {
        for vars in [&workspace, &cross_user] {
            let data = exec_ok(&f.schema, who.request(AUDIT_LOG, vars.clone())).await;
            assert_eq!(bob_rows(&data), 1, "{name} reads the workspace log");
        }
    }

    let olive = f.ctx.create_user("mod-olive").await;
    let olive = f.ctx.caller(&olive).await;
    for (who, name) in [
        (&f.carol, "member"),
        (&f.gina, "guest"),
        (&olive, "outsider"),
    ] {
        for vars in [&workspace, &cross_user] {
            let code = exec_err_code(&f.schema, who.request(AUDIT_LOG, vars.clone())).await;
            assert_eq!(code, "FORBIDDEN", "{name} cannot read the workspace log");
        }
    }

    let code = exec_err_code(
        &f.schema,
        f.carol.request(AUDIT_LOG, json!({ "filter": {} })),
    )
    .await;
    assert_eq!(code, "FORBIDDEN", "a filter with neither field is rejected");

    let own = json!({ "filter": { "serverId": f.server_id, "actorUserId": bob_id } });
    let data = exec_ok(&f.schema, f.bob.request(AUDIT_LOG, own)).await;
    assert_eq!(bob_rows(&data), 1, "a member reads their own rows in W");
    let own_anywhere = json!({ "filter": { "actorUserId": bob_id } });
    let data = exec_ok(&f.schema, f.bob.request(AUDIT_LOG, own_anywhere)).await;
    assert_eq!(
        bob_rows(&data),
        1,
        "a member reads their own rows without W"
    );
    let gina_own = json!({ "filter": { "actorUserId": f.gina.user_id.to_string() } });
    exec_ok(&f.schema, f.gina.request(AUDIT_LOG, gina_own)).await;

    let code = exec_err_code(&f.schema, anonymous_request(AUDIT_LOG, workspace)).await;
    assert_eq!(code, "UNAUTHENTICATED");
}

/// The audit log lists the newest entry first with every field of the row, pages through
/// `after` with `nextCursor` until `hasNextPage` turns false, and narrows by `typedVerb`,
/// `commandId`, `since` and `until`. A malformed cursor or date is `VALIDATION`.
#[tokio::test]
async fn audit_log_pages_newest_first_and_filters_by_verb_and_command() {
    let f = setup().await;
    f.ctx.seed_capabilities().await;
    let post = |text: &str| {
        json!({
            "serverId": f.server_id,
            "args": { "server_id": f.server_id, "conversation_id": f.channel_id, "text": text },
        })
    };
    let list = json!({
        "serverId": f.server_id,
        "commandId": "chat.list_channels",
        "args": { "server_id": f.server_id },
    });
    for vars in [post("first"), list, post("second")] {
        let ok = exec_ok(&f.schema, f.bob.request(EXECUTE, vars)).await;
        assert_eq!(ok["executeCommand"]["status"], json!("SUCCESS"));
    }
    let workspace = json!({ "serverId": f.server_id });
    let commands = |page: &Value| -> Vec<String> {
        page["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|row| row["commandId"].as_str().expect("command id").to_string())
            .collect()
    };

    let all = f.audit_page(workspace.clone(), Value::Null).await;
    assert_eq!(
        commands(&all),
        [
            "chat.post_message",
            "chat.list_channels",
            "chat.post_message"
        ]
    );
    let newest = &all["items"][0];
    assert!(uuid::Uuid::parse_str(newest["id"].as_str().expect("id")).is_ok());
    let created_at: chrono::DateTime<chrono::Utc> = newest["createdAt"]
        .as_str()
        .expect("createdAt")
        .parse()
        .expect("RFC 3339");
    assert!(created_at <= chrono::Utc::now());
    assert_eq!(
        without(newest, &["id", "createdAt"]),
        json!({
            "actorUserId": f.bob.user_id.to_string(),
            "serverId": f.server_id,
            "commandId": "chat.post_message",
            "initiator": "palette",
            "argsRedacted": {
                "server_id": f.server_id, "conversation_id": f.channel_id, "text": "second",
            },
            "sensitivity": "mutating",
            "dangerous": false,
            "confirmationKind": "none",
            "typedVerb": "SEND",
            "outcome": "SUCCESS",
            "errorKind": null,
            "capabilityCheckOutcome": null,
            "confirmationOutcome": null,
        })
    );
    assert_eq!(
        all["pageInfo"],
        json!({ "hasNextPage": false, "hasPrevPage": false })
    );
    assert_eq!(all["nextCursor"], Value::Null);

    let mut seen = Vec::new();
    let mut after = Value::Null;
    loop {
        let page = f
            .audit_page(workspace.clone(), json!({ "after": after, "limit": 1 }))
            .await;
        assert_eq!(page["items"].as_array().expect("items").len(), 1);
        assert_eq!(page["pageInfo"]["hasPrevPage"], json!(!after.is_null()));
        seen.push(page["items"][0]["id"].clone());
        after = page["nextCursor"].clone();
        assert_eq!(page["pageInfo"]["hasNextPage"], json!(!after.is_null()));
        if after.is_null() {
            break;
        }
    }
    let all_ids: Vec<Value> = all["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|row| row["id"].clone())
        .collect();
    assert_eq!(seen, all_ids, "each row once, in the same order");

    let reads = f
        .audit_page(
            json!({ "serverId": f.server_id, "typedVerb": "READ" }),
            Value::Null,
        )
        .await;
    assert_eq!(commands(&reads), ["chat.list_channels"]);
    assert_eq!(reads["items"][0]["typedVerb"], json!("READ"));
    let sends = f
        .audit_page(
            json!({ "serverId": f.server_id, "commandId": "chat.post_message" }),
            Value::Null,
        )
        .await;
    assert_eq!(commands(&sends), ["chat.post_message", "chat.post_message"]);
    let none = f
        .audit_page(
            json!({ "serverId": f.server_id, "typedVerb": "DELETE" }),
            Value::Null,
        )
        .await;
    assert_eq!(commands(&none), Vec::<String>::new());

    let hour = chrono::Duration::hours(1);
    for (bound, at) in [("since", created_at + hour), ("until", created_at - hour)] {
        let filter = json!({ "serverId": f.server_id, bound: at.to_rfc3339() });
        let page = f.audit_page(filter, Value::Null).await;
        assert_eq!(commands(&page), Vec::<String>::new(), "{bound}");
    }
    for (filter, pagination) in [
        (json!({ "since": "yesterday" }), Value::Null),
        (json!({ "until": "2026-13-01" }), Value::Null),
        (json!({}), json!({ "after": "not-a-cursor" })),
    ] {
        let filter = merged(json!({ "serverId": f.server_id }), filter);
        let vars = json!({ "filter": filter, "pagination": pagination });
        let code = exec_err_code(&f.schema, f.alice.request(AUDIT_LOG, vars)).await;
        assert_eq!(code, "VALIDATION", "{filter} {pagination}");
    }
}

/// `row` without the fields named in `drop`.
fn without(row: &Value, drop: &[&str]) -> Value {
    let mut row = row.clone();
    if let Some(fields) = row.as_object_mut() {
        for name in drop {
            fields.remove(*name);
        }
    }
    row
}

#[tokio::test]
async fn sdl_exposes_the_role_enum_and_moderation_surface() {
    let f = setup().await;
    let sdl = f.schema.sdl();
    for needle in [
        "enum ServerMemberRoleGql",
        "MEMBER",
        "memberRole: ServerMemberRoleGql!",
        "timeoutUntil: DateTime",
        "slowModeSeconds: Int!",
        "role: ServerMemberRoleGql",
        "enum MessageReportStatus",
        "enum ReportOutcome",
        "setMemberRole(",
        "kickMember(",
        "banMember(",
        "unbanMember(",
        "timeoutMember(",
        "clearTimeout(",
        "setSlowMode(",
        "reportMessage(",
        "resolveReport(",
        "serverBans(",
        "messageReports(",
    ] {
        assert!(sdl.contains(needle), "SDL is missing `{needle}`");
    }
}
