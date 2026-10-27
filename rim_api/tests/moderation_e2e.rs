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

use std::collections::BTreeSet;

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::server_member::model::ServerMemberRole;
use rim_services::auth::dto::RegistrationMode;

use support::graphql::{
    Call, Caller, anonymous_request, assert_each_refused, assert_each_refused_as, exec_err,
    exec_err_code, exec_ok, merged,
};
use support::roles::create_channel;
use support::{TestCtx, unknown_id};

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

const SET_CHANNEL_POSTING: &str = r#"
mutation Posting($serverId: ID!, $conversationId: ID!, $policy: PostingPolicy!,
                 $allowsThreads: Boolean!) {
  conversation: setChannelPosting(serverId: $serverId, conversationId: $conversationId,
                                  policy: $policy, allowsThreads: $allowsThreads) {
    id postingPolicy postingAllowsThreads
  }
}
"#;

const REPLY: &str = r#"
mutation Reply($input: ReplyToMessageInput!) { msg: replyToMessage(input: $input) { id } }
"#;

const TOGGLE_REACTION: &str = r#"
mutation React($input: ToggleReactionInput!) {
  reactions: toggleReaction(input: $input) { emoji userIds }
}
"#;

const PIN_MESSAGE: &str = "mutation Pin($input: PinMessageInput!) { pinMessage(input: $input) }";

const FORWARD_MESSAGE: &str = r#"
mutation Forward($input: ForwardMessageInput!) { msg: forwardMessage(input: $input) { id } }
"#;

const CREATE_DM: &str = r#"
mutation Dm($serverId: ID!, $memberIds: [ID!]!) {
  conversation: createConversation(input: { serverId: $serverId, memberIds: $memberIds }) { id }
}
"#;

const SET_ARCHIVED: &str = r#"
mutation Archive($serverId: ID!, $conversationId: ID!) {
  conversation: setConversationArchived(serverId: $serverId, conversationId: $conversationId,
                                        archived: true) { id }
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
async fn message_author_ban_status_is_public_and_reason_is_moderator_only() {
    use sea_orm::ConnectionTrait;
    let f = setup().await;
    exec_ok(&f.schema, f.send_request(&f.bob, "before ban")).await;
    let query = MESSAGES_QUERY.replace(
        "items { id }",
        "items { id authorId author { banned bannedUntil banReason } }",
    );
    let vars = json!({ "serverId": f.server_id, "conversationId": f.channel_id });
    exec_ok(&f.schema, f.mona.request(BAN, f.member_vars(&f.bob))).await;
    let status = exec_ok(&f.schema, f.carol.request(&query, vars.clone())).await;
    let author = &status["page"]["items"][0]["author"];
    assert_eq!(author["banned"], true);
    assert!(author["bannedUntil"].is_null());
    assert!(author["banReason"].is_null());
    let status = exec_ok(&f.schema, f.mona.request(&query, vars.clone())).await;
    assert_eq!(status["page"]["items"][0]["author"]["banReason"], "spam");
    exec_ok(&f.schema, f.mona.request(UNBAN, f.member_vars(&f.bob))).await;
    let status = exec_ok(&f.schema, f.carol.request(&query, vars.clone())).await;
    assert_eq!(status["page"]["items"][0]["author"]["banned"], false);
    exec_ok(&f.schema, f.mona.request(BAN, f.member_vars(&f.bob))).await;
    f.ctx.conn.execute_unprepared(&format!("UPDATE server_ban SET expires_at = now() - interval '1 second' WHERE server_id = '{}' AND user_id = '{}'", f.server_id, f.bob.user_id)).await.unwrap();
    let status = exec_ok(&f.schema, f.carol.request(&query, vars)).await;
    assert_eq!(status["page"]["items"][0]["author"]["banned"], false);
}

#[tokio::test]
async fn author_ban_status_is_scoped_to_each_workspace_in_one_query() {
    use rim_core::user::repository::UserRepository;
    use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
    let f = setup().await;
    let users = SeaOrmUserRepository::new(f.ctx.db.clone());
    let bob = users.get_by_id(f.bob.user_id).await.unwrap().unwrap();
    let alice = users.get_by_id(f.alice.user_id).await.unwrap().unwrap();
    let other = f.ctx.create_server("other workspace").await;
    f.ctx.add_owner(&other, &alice).await;
    f.ctx
        .add_member(&other, &bob, ServerMemberRole::Member)
        .await;
    let channel =
        create_channel(&f.schema, &f.alice, &other.id.to_string(), "general", false).await;
    exec_ok(&f.schema, f.bob.request(SEND_MESSAGE, json!({"input": {"serverId":other.id.to_string(), "conversationId":channel, "text":"another workspace"}}))).await;
    exec_ok(&f.schema, f.send_request(&f.bob, "first workspace")).await;
    exec_ok(&f.schema, f.mona.request(BAN, f.member_vars(&f.bob))).await;
    let query = format!(
        r#"query {{
        first: messages(serverId:"{}", conversationId:"{}", dir:AFTER, limit:10, includeDeleted:false) {{ items {{ author {{ banned bannedUntil }} }} }}
        second: messages(serverId:"{}", conversationId:"{}", dir:AFTER, limit:10, includeDeleted:false) {{ items {{ author {{ banned bannedUntil }} }} }}
    }}"#,
        f.server_id, f.channel_id, other.id, channel
    );
    let result = exec_ok(&f.schema, f.alice.request(&query, json!({}))).await;
    assert_eq!(result["first"]["items"][0]["author"]["banned"], true);
    assert_eq!(result["second"]["items"][0]["author"]["banned"], false);
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

impl Fixture {
    /// `setChannelPosting` on `conversation_id` as `who`.
    fn posting_request(
        &self,
        who: &Caller,
        conversation_id: &str,
        policy: &str,
        allows_threads: bool,
    ) -> async_graphql::Request {
        who.request(
            SET_CHANNEL_POSTING,
            json!({
                "serverId": self.server_id,
                "conversationId": conversation_id,
                "policy": policy,
                "allowsThreads": allows_threads,
            }),
        )
    }

    /// `sendMessage` of `text` into `conversation_id` as `who`.
    fn send_into(&self, who: &Caller, conversation_id: &str, text: &str) -> async_graphql::Request {
        who.request(
            SEND_MESSAGE,
            json!({ "input": {
                "serverId": self.server_id, "conversationId": conversation_id, "text": text,
            } }),
        )
    }

    /// `replyToMessage` in the thread of `root` in `conversation_id` as `who`.
    fn reply(&self, who: &Caller, conversation_id: &str, root: &str) -> async_graphql::Request {
        who.request(
            REPLY,
            json!({ "input": {
                "serverId": self.server_id, "conversationId": conversation_id,
                "rootMessageId": root, "text": "in the thread",
            } }),
        )
    }

    /// `toggleReaction` of `emoji` on `message` in `conversation_id` as `who`.
    fn react(
        &self,
        who: &Caller,
        conversation_id: &str,
        message: &str,
        emoji: &str,
    ) -> async_graphql::Request {
        who.request(
            TOGGLE_REACTION,
            json!({ "input": {
                "serverId": self.server_id, "conversationId": conversation_id,
                "messageId": message, "emoji": emoji,
            } }),
        )
    }
}

#[tokio::test]
async fn a_moderators_only_channel_takes_posts_from_staff_only() {
    let f = setup().await;
    let news = create_channel(&f.schema, &f.alice, &f.server_id, "news", false).await;
    exec_ok(
        &f.schema,
        f.bob.request(
            JOIN_CHANNEL,
            json!({ "serverId": f.server_id, "conversationId": news }),
        ),
    )
    .await;

    let data = exec_ok(
        &f.schema,
        f.posting_request(&f.alice, &news, "MODERATORS", true),
    )
    .await;
    assert_eq!(data["conversation"]["postingPolicy"], json!("MODERATORS"));
    assert_eq!(data["conversation"]["postingAllowsThreads"], json!(true));

    let (code, message) = exec_err(&f.schema, f.send_into(&f.bob, &news, "hello")).await;
    assert_eq!(code, "FORBIDDEN");
    assert!(
        message.contains("only the owner and moderators can post in this channel"),
        "{message}"
    );
    let root = f.send(&f.alice, &news, "release notes").await;
    f.send(&f.mona, &news, "a moderator posts too").await;

    // Threads open: bob reacts and replies in the thread.
    exec_ok(&f.schema, f.react(&f.bob, &news, &root, "👍")).await;
    exec_ok(&f.schema, f.reply(&f.bob, &news, &root)).await;

    let pin = f.bob.request(
        PIN_MESSAGE,
        json!({ "input": { "serverId": f.server_id, "conversationId": news, "messageId": root } }),
    );
    assert_eq!(exec_err_code(&f.schema, pin).await, "FORBIDDEN");

    let elsewhere = f.send(&f.bob, &f.channel_id, "worth sharing").await;
    let forward = f.bob.request(
        FORWARD_MESSAGE,
        json!({ "input": {
            "serverId": f.server_id, "sourceConversationId": f.channel_id,
            "targetConversationId": news, "messageId": elsewhere,
        } }),
    );
    assert_eq!(exec_err_code(&f.schema, forward).await, "FORBIDDEN");

    // Threads closed: no new reaction and no thread reply, but bob may take back his own.
    exec_ok(
        &f.schema,
        f.posting_request(&f.mona, &news, "MODERATORS", false),
    )
    .await;
    assert_eq!(
        exec_err_code(&f.schema, f.react(&f.bob, &news, &root, "🎉")).await,
        "FORBIDDEN"
    );
    assert_eq!(
        exec_err_code(&f.schema, f.reply(&f.bob, &news, &root)).await,
        "FORBIDDEN"
    );
    let data = exec_ok(&f.schema, f.react(&f.bob, &news, &root, "👍")).await;
    assert_eq!(data["reactions"], json!([]));

    exec_ok(
        &f.schema,
        f.posting_request(&f.alice, &news, "EVERYONE", true),
    )
    .await;
    exec_ok(&f.schema, f.send_into(&f.bob, &news, "hello at last")).await;
    assert_eq!(f.audit_rows("chat.set_channel_posting").await.len(), 3);
}

#[tokio::test]
async fn setting_channel_posting_needs_a_moderator_and_a_channel() {
    let f = setup().await;
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.posting_request(&f.bob, &f.channel_id, "MODERATORS", true)
        )
        .await,
        "FORBIDDEN"
    );

    let dm = exec_ok(
        &f.schema,
        f.bob.request(
            CREATE_DM,
            json!({ "serverId": f.server_id, "memberIds": [f.carol.user_id.to_string()] }),
        ),
    )
    .await;
    let dm = dm["conversation"]["id"]
        .as_str()
        .expect("dm id")
        .to_string();
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.posting_request(&f.alice, &dm, "MODERATORS", true)
        )
        .await,
        "VALIDATION"
    );

    let old = create_channel(&f.schema, &f.alice, &f.server_id, "old", false).await;
    exec_ok(
        &f.schema,
        f.alice.request(
            SET_ARCHIVED,
            json!({ "serverId": f.server_id, "conversationId": old }),
        ),
    )
    .await;
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.posting_request(&f.mona, &old, "MODERATORS", true)
        )
        .await,
        "CONVERSATION_ARCHIVED"
    );

    // Setting what is already set writes no audit row.
    let data = exec_ok(
        &f.schema,
        f.posting_request(&f.alice, &f.channel_id, "EVERYONE", true),
    )
    .await;
    assert_eq!(data["conversation"]["postingPolicy"], json!("EVERYONE"));
    assert!(f.audit_rows("chat.set_channel_posting").await.is_empty());
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

    let olive = f.outsider().await;
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

// ── The checklist: who may call each root, and what each refusal answers ─────────────────────

const MESSAGE_REPORT: &str = r#"
query Report($serverId: ID!, $reportId: ID!) {
  report: messageReport(serverId: $serverId, reportId: $reportId) { id status }
}
"#;

const BAN_WITH: &str = r#"
mutation Ban($serverId: ID!, $userId: ID!, $reason: String, $expiresAt: DateTime) {
  ban: banMember(serverId: $serverId, userId: $userId, reason: $reason, expiresAt: $expiresAt) {
    userId
  }
}
"#;

const MY_SERVERS: &str = "query { myServers { id } }";

const SERVER: &str = r#"
query Server($serverId: ID!) { server(serverId: $serverId) { id name } }
"#;

const CREATE_SERVER: &str = r#"
mutation CreateServer($name: String!) { server: createServer(input: { name: $name }) { id name } }
"#;

const LEAVE_WORKSPACE: &str = r#"
mutation LeaveWorkspace($serverId: ID!) { leaveWorkspace(serverId: $serverId) }
"#;

const RENAME_WORKSPACE: &str = r#"
mutation RenameWorkspace($serverId: ID!, $name: String!) { renameWorkspace(serverId: $serverId, name: $name) { id name } }
"#;

/// The ids a moderation call names.
#[derive(Clone)]
struct Ids {
    server: String,
    user: String,
    conversation: String,
    message: String,
    report: String,
}

/// `minutes` from now, as GraphQL `DateTime`.
fn in_minutes(minutes: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::minutes(minutes)).to_rfc3339()
}

/// One call to each root that acts on the member `user` of `server`: kick, ban, time out, clear
/// the timeout.
fn targeting(server: &str, user: &str) -> Vec<Call> {
    let member = json!({ "serverId": server, "userId": user });
    vec![
        (KICK, member.clone()),
        (BAN, member.clone()),
        (
            TIMEOUT,
            merged(member.clone(), json!({ "until": in_minutes(10) })),
        ),
        (CLEAR_TIMEOUT, member),
    ]
}

/// One call to each moderation root a Moderator or Owner may make, naming `ids`.
fn moderator_roots(ids: &Ids) -> Vec<Call> {
    let server = json!({ "serverId": ids.server });
    let report = merged(server.clone(), json!({ "reportId": ids.report }));
    let mut calls = vec![
        (BANS, server.clone()),
        (REPORTS, server.clone()),
        (MESSAGE_REPORT, report.clone()),
        (RESOLVE, merged(report, json!({ "outcome": "DISMISSED" }))),
        (
            SET_MEMBER_ROLE,
            merged(
                server.clone(),
                json!({ "userId": ids.user, "role": "GUEST" }),
            ),
        ),
        (UNBAN, merged(server.clone(), json!({ "userId": ids.user }))),
        (
            SET_SLOW_MODE,
            merged(
                server,
                json!({ "conversationId": ids.conversation, "seconds": 60 }),
            ),
        ),
    ];
    calls.extend(targeting(&ids.server, &ids.user));
    calls
}

/// The `reportMessage` call naming `ids`; any member who reads the message may make it.
fn report_call(ids: &Ids) -> Call {
    (
        REPORT,
        json!({
            "serverId": ids.server, "conversationId": ids.conversation,
            "messageId": ids.message, "reason": "spam",
        }),
    )
}

/// One call to every moderation root, naming `ids`.
fn every_moderation_root(ids: &Ids) -> Vec<Call> {
    let mut calls = moderator_roots(ids);
    calls.push(report_call(ids));
    calls
}

/// One call to every workspace root that names the workspace `server`.
fn workspace_roots(server: &str) -> Vec<Call> {
    let server = json!({ "serverId": server });
    vec![
        (SERVER, server.clone()),
        (SERVER_MEMBERS, server.clone()),
        (LEAVE_WORKSPACE, server.clone()),
        (RENAME_WORKSPACE, merged(server, json!({ "name": "x" }))),
    ]
}

impl Fixture {
    /// A user who belongs to no workspace.
    async fn outsider(&self) -> Caller {
        let olive = self.ctx.create_user("mod-olive").await;
        self.ctx.caller(&olive).await
    }

    /// Ids of a message bob sent to the channel, carol's report of it, and carol as the target.
    async fn reported(&self) -> Ids {
        let message = self.send(&self.bob, &self.channel_id, "report me").await;
        let mut ids = Ids {
            server: self.server_id.clone(),
            user: self.carol.user_id.to_string(),
            conversation: self.channel_id.clone(),
            message,
            report: String::new(),
        };
        let data = exec_ok(&self.schema, self.carol.call(report_call(&ids))).await;
        ids.report = data["report"]["id"]
            .as_str()
            .expect("report id")
            .to_string();
        ids
    }

    async fn code(&self, who: &Caller, call: Call) -> String {
        exec_err_code(&self.schema, who.call(call)).await
    }
}

#[tokio::test]
async fn every_moderation_and_workspace_root_refuses_an_anonymous_caller() {
    let f = setup().await;
    let ids = f.reported().await;
    let mut calls = every_moderation_root(&ids);
    calls.extend(workspace_roots(&f.server_id));
    calls.push((MY_SERVERS, json!({})));
    calls.push((CREATE_SERVER, json!({ "name": "mine" })));
    assert_each_refused(&f.schema, &calls, "UNAUTHENTICATED", anonymous_request).await;
}

/// An outsider and a caller naming a workspace that does not exist get the same answer.
#[tokio::test]
async fn every_moderation_and_workspace_root_refuses_an_outsider_and_an_unknown_workspace() {
    let f = setup().await;
    let ids = f.reported().await;
    let mut calls = every_moderation_root(&ids);
    calls.extend(workspace_roots(&f.server_id));
    assert_each_refused_as(&f.schema, &f.outsider().await, &calls, "FORBIDDEN").await;

    let elsewhere = Ids {
        server: unknown_id(),
        ..ids
    };
    let mut calls = every_moderation_root(&elsewhere);
    calls.extend(workspace_roots(&elsewhere.server));
    assert_each_refused_as(&f.schema, &f.alice, &calls, "FORBIDDEN").await;
}

#[tokio::test]
async fn members_and_guests_may_not_moderate() {
    let f = setup().await;
    let ids = f.reported().await;
    for who in [&f.bob, &f.gina] {
        assert_each_refused_as(&f.schema, who, &moderator_roots(&ids), "FORBIDDEN").await;
    }
    assert!(
        f.member_ids(&f.alice)
            .await
            .contains(&f.carol.user_id.to_string())
    );
    let page = exec_ok(
        &f.schema,
        f.mona.request(
            REPORTS,
            json!({ "serverId": f.server_id, "status": "OPEN" }),
        ),
    )
    .await;
    assert_eq!(page["page"]["items"][0]["id"], json!(ids.report));
}

#[tokio::test]
async fn every_moderation_and_workspace_root_refuses_a_malformed_id() {
    let f = setup().await;
    let ids = f.reported().await;
    let bad_server = Ids {
        server: "not-a-uuid".to_string(),
        ..ids.clone()
    };
    let mut calls = every_moderation_root(&bad_server);
    calls.extend(workspace_roots(&bad_server.server));
    assert_each_refused_as(&f.schema, &f.alice, &calls, "VALIDATION").await;

    let bad_rest = Ids {
        server: ids.server,
        user: "not-a-uuid".to_string(),
        conversation: "not-a-uuid".to_string(),
        message: "not-a-uuid".to_string(),
        report: "not-a-uuid".to_string(),
    };
    let calls: Vec<Call> = every_moderation_root(&bad_rest)
        .into_iter()
        .filter(|(query, _)| ![BANS, REPORTS].contains(query))
        .collect();
    assert_each_refused_as(&f.schema, &f.alice, &calls, "VALIDATION").await;
}

/// An unknown member, report, conversation or message is `NOT_FOUND`, and so is a report or a
/// message of another workspace or conversation named under this one. Lifting a ban or a timeout
/// that is not there answers `false`.
#[tokio::test]
async fn moderation_roots_answer_not_found_for_an_unknown_target() {
    let f = setup().await;
    let ids = f.reported().await;
    let unknown = Ids {
        server: f.server_id.clone(),
        user: unknown_id(),
        conversation: unknown_id(),
        message: unknown_id(),
        report: unknown_id(),
    };
    let calls: Vec<Call> = every_moderation_root(&unknown)
        .into_iter()
        .filter(|(query, _)| ![BANS, REPORTS, UNBAN, CLEAR_TIMEOUT].contains(query))
        .collect();
    assert_each_refused_as(&f.schema, &f.mona, &calls, "NOT_FOUND").await;
    for query in [UNBAN, CLEAR_TIMEOUT] {
        let data = exec_ok(
            &f.schema,
            f.mona.request(
                query,
                json!({ "serverId": f.server_id, "userId": unknown.user }),
            ),
        )
        .await;
        assert_eq!(
            data.as_object().expect("data").values().next(),
            Some(&json!(false))
        );
    }

    // A report of olive's workspace, named under this one.
    let olive_user = f.ctx.create_user("mod-olive").await;
    let other = f.ctx.create_server("moderation-e2e-other").await;
    f.ctx.add_owner(&other, &olive_user).await;
    let olive = f.ctx.caller(&olive_user).await;
    let other_id = other.id.to_string();
    let other_channel = create_channel(&f.schema, &olive, &other_id, "elsewhere", false).await;
    let theirs = exec_ok(
        &f.schema,
        olive.request(
            SEND_MESSAGE,
            json!({ "input": {
                "serverId": other_id, "conversationId": other_channel, "text": "x",
            } }),
        ),
    )
    .await;
    let foreign = Ids {
        server: other_id,
        user: f.carol.user_id.to_string(),
        conversation: other_channel,
        message: theirs["msg"]["id"].as_str().expect("id").to_string(),
        report: String::new(),
    };
    let data = exec_ok(&f.schema, olive.call(report_call(&foreign))).await;
    let here = json!({ "serverId": f.server_id,
                       "reportId": data["report"]["id"].as_str().expect("report id") });
    let calls = [
        (MESSAGE_REPORT, here.clone()),
        (RESOLVE, merged(here, json!({ "outcome": "RESOLVED" }))),
    ];
    assert_each_refused_as(&f.schema, &f.mona, &calls, "NOT_FOUND").await;

    // A message of the channel, named under another channel of the workspace.
    let second = create_channel(&f.schema, &f.alice, &f.server_id, "second", false).await;
    let misplaced = Ids {
        conversation: second,
        ..ids
    };
    assert_eq!(f.code(&f.mona, report_call(&misplaced)).await, "NOT_FOUND");
}

/// Kick, ban, timeout and clearing it refuse the caller themselves (`VALIDATION`), the owner and,
/// for a moderator, a fellow moderator (`FORBIDDEN`).
#[tokio::test]
async fn nobody_moderates_themselves_the_owner_or_a_fellow_moderator() {
    let f = setup().await;
    let alice = f.alice.user_id.to_string();
    let mona = f.mona.user_id.to_string();
    for (who, me) in [(&f.alice, &alice), (&f.mona, &mona)] {
        assert_each_refused_as(&f.schema, who, &targeting(&f.server_id, me), "VALIDATION").await;
    }
    assert_each_refused_as(
        &f.schema,
        &f.mona,
        &targeting(&f.server_id, &alice),
        "FORBIDDEN",
    )
    .await;

    exec_ok(
        &f.schema,
        f.alice.request(
            SET_MEMBER_ROLE,
            json!({ "serverId": f.server_id, "userId": f.bob.user_id.to_string(),
                    "role": "MODERATOR" }),
        ),
    )
    .await;
    let bob = f.bob.user_id.to_string();
    assert_each_refused_as(
        &f.schema,
        &f.mona,
        &targeting(&f.server_id, &bob),
        "FORBIDDEN",
    )
    .await;
    assert_eq!(f.member_ids(&f.alice).await.len(), 5, "nobody was removed");
}

#[tokio::test]
async fn moderation_roots_refuse_out_of_range_times_reasons_and_conversations() {
    let f = setup().await;
    let ids = f.reported().await;
    let carol = f.member_vars(&f.carol);
    let group = exec_ok(
        &f.schema,
        f.alice.request(
            CREATE_CHANNEL,
            json!({ "input": { "serverId": f.server_id, "isChannel": false, "title": "g",
                               "memberIds": [f.carol.user_id.to_string()] } }),
        ),
    )
    .await;
    let group_id = group["conversation"]["id"].as_str().expect("group id");
    let slow = |conversation: &str, seconds: i64| {
        (
            SET_SLOW_MODE,
            json!({ "serverId": f.server_id, "conversationId": conversation, "seconds": seconds }),
        )
    };
    let calls = [
        (
            BAN_WITH,
            merged(carol.clone(), json!({ "expiresAt": in_minutes(-1) })),
        ),
        (
            BAN_WITH,
            merged(carol.clone(), json!({ "reason": "r".repeat(513) })),
        ),
        (
            TIMEOUT,
            merged(carol.clone(), json!({ "until": in_minutes(-1) })),
        ),
        (
            TIMEOUT,
            merged(
                carol.clone(),
                json!({ "until": in_minutes(30 * 24 * 60 + 5) }),
            ),
        ),
        slow(&f.channel_id, -1),
        slow(group_id, 60),
    ];
    assert_each_refused_as(&f.schema, &f.mona, &calls, "VALIDATION").await;

    let report = |reason: String| {
        let (query, vars) = report_call(&ids);
        (query, merged(vars, json!({ "reason": reason })))
    };
    let calls = [report("  ".to_string()), report("r".repeat(1001))];
    assert_each_refused_as(&f.schema, &f.bob, &calls, "VALIDATION").await;

    let data = exec_ok(
        &f.schema,
        f.mona.request(
            BAN_WITH,
            merged(carol, json!({ "reason": "r".repeat(512) })),
        ),
    )
    .await;
    assert_eq!(data["ban"]["userId"], json!(f.carol.user_id.to_string()));
}

#[tokio::test]
async fn create_server_refuses_a_blank_or_overlong_name() {
    let f = setup().await;
    let calls = [
        (CREATE_SERVER, json!({ "name": "   " })),
        (CREATE_SERVER, json!({ "name": "n".repeat(129) })),
    ];
    assert_each_refused_as(&f.schema, &f.bob, &calls, "VALIDATION").await;
    let data = exec_ok(
        &f.schema,
        f.bob.request(
            CREATE_SERVER,
            json!({ "name": format!(" {} ", "n".repeat(128)) }),
        ),
    )
    .await;
    assert_eq!(data["server"]["name"], json!("n".repeat(128)));
    let mine = exec_ok(&f.schema, f.bob.request(MY_SERVERS, json!({}))).await;
    assert_eq!(mine["myServers"].as_array().expect("servers").len(), 2);
}

/// One person may not own two workspaces with the same name; anyone else may reuse it, a mere
/// member of the first included.
#[tokio::test]
async fn create_server_refuses_a_name_the_caller_already_owns() {
    let f = setup().await;
    let create = |who: &Caller, name: &str| who.request(CREATE_SERVER, json!({ "name": name }));
    let sorted = |mut ids: Vec<Value>| {
        ids.sort_by_key(Value::to_string);
        ids
    };
    let mine = async |who: &Caller| {
        let data = exec_ok(&f.schema, who.request(MY_SERVERS, json!({}))).await;
        let servers = data["myServers"].as_array().expect("servers").clone();
        sorted(servers.into_iter().map(|s| s["id"].clone()).collect())
    };

    let design = exec_ok(&f.schema, create(&f.alice, "Design")).await["server"]["id"].clone();
    for name in ["Design", "  design "] {
        let (code, message) = exec_err(&f.schema, create(&f.alice, name)).await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("CONFLICT", r#"you already own a workspace named "Design""#),
            "{name:?}"
        );
    }

    let design_id = design.as_str().expect("server id");
    f.ctx
        .join(design_id, &f.bob, ServerMemberRole::Member)
        .await;
    let bobs = exec_ok(&f.schema, create(&f.bob, "Design")).await["server"]["id"].clone();
    assert_ne!(bobs, design);

    let home = json!(f.server_id);
    assert_eq!(
        mine(&f.alice).await,
        sorted(vec![home.clone(), design.clone()])
    );
    assert_eq!(mine(&f.bob).await, sorted(vec![home, design, bobs]));
}

// ── User commands and command discovery ───────────────────────────────────────────────────────

const CREATE_USER_COMMAND: &str = r#"
mutation Create($input: CreateUserCommandInput!) {
  command: createUserCommand(input: $input) { id title }
}
"#;

const UPDATE_USER_COMMAND: &str = r#"
mutation Update($input: UpdateUserCommandInput!) {
  command: updateUserCommand(input: $input) { id title }
}
"#;

const DELETE_USER_COMMAND: &str = r#"
mutation Delete($commandId: String!) { deleted: deleteUserCommand(input: { commandId: $commandId }) }
"#;

const COMMAND_DISCOVER: &str = r#"
query Discover($workspaceId: ID) { commandDiscover(workspaceId: $workspaceId, limit: 100) { id } }
"#;

const PALETTE_COMMANDS: &str = r#"
query Palette($context: CommandContextInput) { commands(commandContext: $context) { id } }
"#;

/// A user command `key` titled `title` that runs `step`.
fn user_command(key: &str, title: &str, step: &str) -> Value {
    json!({ "commandId": key, "title": title, "description": "", "steps": [{ "commandId": step }] })
}

fn create_command(key: &str, step: &str) -> Call {
    (
        CREATE_USER_COMMAND,
        json!({ "input": user_command(key, "made", step) }),
    )
}

fn update_command(key: &str) -> Call {
    let input = merged(
        user_command(key, "changed", "chat.post_message"),
        json!({ "isEnabled": true }),
    );
    (UPDATE_USER_COMMAND, json!({ "input": input }))
}

fn delete_command(key: &str) -> Call {
    (DELETE_USER_COMMAND, json!({ "commandId": key }))
}

/// One call to every user command root, naming the command `key`.
fn every_user_command_root(key: &str) -> Vec<Call> {
    vec![
        create_command(key, "chat.post_message"),
        update_command(key),
        delete_command(key),
    ]
}

impl Fixture {
    /// The palette command ids `who` sees, with `workspace` as the context when given.
    async fn palette(&self, who: &Caller, workspace: Option<&str>) -> BTreeSet<String> {
        let context = workspace.map(|id| json!({ "workspaceId": id }));
        let data = exec_ok(
            &self.schema,
            who.request(PALETTE_COMMANDS, json!({ "context": context })),
        )
        .await;
        data["commands"]
            .as_array()
            .expect("commands")
            .iter()
            .map(|c| c["id"].as_str().expect("id").to_string())
            .collect()
    }

    /// The ids `who` discovers, scoped to `workspace` when given.
    async fn discovered(&self, who: &Caller, workspace: Option<&str>) -> Vec<String> {
        let data = exec_ok(
            &self.schema,
            who.request(COMMAND_DISCOVER, json!({ "workspaceId": workspace })),
        )
        .await;
        data["commandDiscover"]
            .as_array()
            .expect("commands")
            .iter()
            .map(|c| c["id"].as_str().expect("id").to_string())
            .collect()
    }
}

#[tokio::test]
async fn every_command_root_refuses_an_anonymous_caller() {
    let f = setup().await;
    exec_ok(
        &f.schema,
        f.bob
            .call(create_command("user.greet", "chat.post_message")),
    )
    .await;
    let mut calls = every_user_command_root("user.greet");
    calls.push((COMMAND_DISCOVER, json!({ "workspaceId": f.server_id })));
    assert_each_refused(&f.schema, &calls, "UNAUTHENTICATED", anonymous_request).await;
}

/// User commands are per account: another member and the workspace owner see no command of
/// bob's (`NOT_FOUND`) and may name their own command the same, and a step may name only the
/// caller's own user commands.
#[tokio::test]
async fn a_user_command_belongs_to_its_creator_alone() {
    let f = setup().await;
    exec_ok(
        &f.schema,
        f.bob
            .call(create_command("user.greet", "chat.post_message")),
    )
    .await;
    let changes = [update_command("user.greet"), delete_command("user.greet")];
    for who in [&f.carol, &f.alice] {
        assert_each_refused_as(&f.schema, who, &changes, "NOT_FOUND").await;
    }
    let (code, message) = exec_err(
        &f.schema,
        f.carol.call(create_command("user.borrowed", "user.greet")),
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        ("VALIDATION", "unknown command step `user.greet`")
    );

    let theirs = exec_ok(
        &f.schema,
        f.carol
            .call(create_command("user.greet", "chat.list_channels")),
    )
    .await;
    assert_eq!(theirs["command"]["id"], json!("user.greet"));
    let mine = exec_ok(&f.schema, f.bob.call(update_command("user.greet"))).await;
    assert_eq!(mine["command"]["title"], json!("changed"));
    let deleted = exec_ok(&f.schema, f.bob.call(delete_command("user.greet"))).await;
    assert_eq!(deleted["deleted"], json!(true));
    assert_each_refused_as(&f.schema, &f.bob, &changes, "NOT_FOUND").await;
    let kept = exec_ok(&f.schema, f.carol.call(update_command("user.greet"))).await;
    assert_eq!(kept["command"]["title"], json!("changed"));
}

/// An id outside the `user.` namespace is `VALIDATION` on create and, like an unknown id,
/// `NOT_FOUND` on update and delete.
#[tokio::test]
async fn user_command_roots_refuse_an_unknown_or_malformed_id() {
    let f = setup().await;
    let calls = [
        update_command("user.nothing"),
        delete_command("user.nothing"),
        update_command("greet"),
        delete_command("greet"),
    ];
    assert_each_refused_as(&f.schema, &f.bob, &calls, "NOT_FOUND").await;
    let call = create_command("greet", "chat.post_message");
    assert_eq!(f.code(&f.bob, call).await, "VALIDATION");
}

/// `commands` and `commandDiscover` list what the caller may run. A command the server runs is
/// listed only to a member of the context's workspace whose role may run it; the commands the
/// client runs itself are listed to everyone, in a workspace or without one.
#[tokio::test]
async fn command_listings_follow_membership_and_role() {
    let f = setup().await;
    f.ctx.seed_capabilities().await;
    let olive = f.outsider().await;

    let client_run = f.palette(&f.bob, None).await;
    assert!(client_run.contains("workspace.open_settings"));
    assert!(!client_run.contains("chat.post_message"));
    assert_eq!(f.palette(&olive, Some(&f.server_id)).await, client_run);

    let member = [
        "chat.create_channel",
        "chat.create_group",
        "chat.invite_to_conversation",
        "chat.join_channel",
    ];
    let moderator = [
        "admin.ban",
        "admin.kick",
        "admin.set_role",
        "admin.timeout",
        "admin.unban",
        "chat.archive_channel",
        "chat.set_channel_posting",
        "chat.set_slow_mode",
        "chat.unarchive_channel",
    ];
    let owner = ["admin.invite_member"];
    let role_bound: BTreeSet<String> = member
        .iter()
        .chain(&moderator)
        .chain(&owner)
        .map(|id| id.to_string())
        .collect();
    let set = |groups: &[&[&str]]| -> BTreeSet<String> {
        groups
            .iter()
            .flat_map(|ids| ids.iter().map(|id| id.to_string()))
            .collect()
    };
    for (who, sees) in [
        (&f.gina, set(&[])),
        (&f.bob, set(&[&member])),
        (&f.mona, set(&[&member, &moderator])),
        (&f.alice, set(&[&member, &moderator, &owner])),
    ] {
        let ids = f.palette(who, Some(&f.server_id)).await;
        assert!(client_run.is_subset(&ids), "{:?}", who.user_id);
        assert!(ids.contains("chat.post_message"), "{:?}", who.user_id);
        let bound: BTreeSet<String> = ids.intersection(&role_bound).cloned().collect();
        assert_eq!(bound, sees, "{:?}", who.user_id);
    }

    let moderation = ["admin.timeout", "chat.set_slow_mode"];
    for (who, moderates) in [
        (&f.gina, false),
        (&f.bob, false),
        (&f.mona, true),
        (&f.alice, true),
    ] {
        let ids = f.discovered(who, Some(&f.server_id)).await;
        assert!(ids.contains(&"chat.post_message".to_string()));
        for id in moderation {
            assert_eq!(
                ids.contains(&id.to_string()),
                moderates,
                "{id} for {:?}",
                who.user_id
            );
        }
    }
    for (who, workspace) in [(&olive, Some(f.server_id.as_str())), (&f.bob, None)] {
        let ids = f.discovered(who, workspace).await;
        assert!(ids.contains(&"workspace.open_settings".to_string()));
        for id in ["state.get", "chat.post_message", "admin.timeout"] {
            assert!(!ids.contains(&id.to_string()), "{id} for {workspace:?}");
        }
    }

    let call = (COMMAND_DISCOVER, json!({ "workspaceId": "not-a-uuid" }));
    assert_eq!(f.code(&f.bob, call).await, "VALIDATION");
}
