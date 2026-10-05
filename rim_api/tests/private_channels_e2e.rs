//! Through-the-stack e2e tests for Track 1.1: private channels, channel join/leave/invite, and
//! archiving (`docs/specs/track-1-1-private-channels.md`).
//!
//! Every assertion goes through GraphQL: resolver → `ConversationService` / `MessageService` /
//! `PinService` → SeaORM repositories → Postgres. The rules under test:
//!
//! - A workspace member who is not in a private channel gets `NOT_FOUND` (never `FORBIDDEN`)
//!   everywhere, and the channel is absent from `channels` and `searchMessages`.
//! - Public channels are joinable; `isMember` and the `joinedOnly` filter reflect membership.
//! - Archived channels are read-only (`CONVERSATION_ARCHIVED`) and hidden from the default
//!   channel list, but listed with `includeArchived`.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use sea_orm::{ConnectionTrait, Statement};
use serde_json::{Value, json};

use rim_api::AppSchema;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::User;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{Caller, exec_err_code, exec_ok};

const CONVERSATION_FIELDS: &str = r#"
fragment ConversationFields on Conversation {
  id title conversationType isPrivate isArchived archivedAt isMember memberCount memberIds createdBy
}
"#;

const CREATE_CONVERSATION: &str = r#"
mutation CreateConversation($input: CreateConversationInput!) {
  conversation: createConversation(input: $input) { ...ConversationFields }
}
"#;

const CONVERSATION_QUERY: &str = r#"
query Conversation($serverId: ID!, $conversationId: ID!) {
  conversation(serverId: $serverId, conversationId: $conversationId) { ...ConversationFields }
}
"#;

const CHANNELS_QUERY: &str = r#"
query Channels($serverId: ID!, $filter: ChannelListFilter) {
  channels(serverId: $serverId, dir: AFTER, limit: 50, filter: $filter) {
    items { ...ConversationFields }
  }
}
"#;

const JOIN_CHANNEL: &str = r#"
mutation Join($serverId: ID!, $conversationId: ID!) {
  conversation: joinChannel(serverId: $serverId, conversationId: $conversationId) {
    ...ConversationFields
  }
}
"#;

const LEAVE_CONVERSATION: &str = r#"
mutation Leave($serverId: ID!, $conversationId: ID!) {
  leaveConversation(serverId: $serverId, conversationId: $conversationId)
}
"#;

const ADD_MEMBERS: &str = r#"
mutation Add($serverId: ID!, $conversationId: ID!, $userIds: [ID!]!) {
  conversation: addConversationMembers(serverId: $serverId, conversationId: $conversationId,
                                       userIds: $userIds) {
    ...ConversationFields
  }
}
"#;

const REMOVE_MEMBER: &str = r#"
mutation Remove($serverId: ID!, $conversationId: ID!, $userId: ID!) {
  removeConversationMember(serverId: $serverId, conversationId: $conversationId, userId: $userId)
}
"#;

const SET_ARCHIVED: &str = r#"
mutation Archive($serverId: ID!, $conversationId: ID!, $archived: Boolean!) {
  conversation: setConversationArchived(serverId: $serverId, conversationId: $conversationId,
                                        archived: $archived) {
    ...ConversationFields
  }
}
"#;

const MESSAGES_QUERY: &str = r#"
query Messages($serverId: ID!, $conversationId: ID!) {
  page: messages(serverId: $serverId, conversationId: $conversationId, dir: AFTER, limit: 20,
                 includeDeleted: false) {
    items { id text }
  }
}
"#;

const PINNED_QUERY: &str = r#"
query Pinned($serverId: ID!, $conversationId: ID!) {
  pinnedMessages(serverId: $serverId, conversationId: $conversationId) { id }
}
"#;

const SEARCH_QUERY: &str = r#"
query Search($input: SearchMessagesInput!) {
  page: searchMessages(input: $input) { items { id text } }
}
"#;

const SEND_MESSAGE: &str = r#"
mutation Send($input: SendMessageInput!) {
  msg: sendMessage(input: $input) { id text }
}
"#;

const PIN_MESSAGE: &str = r#"
mutation Pin($input: PinMessageInput!) { pinMessage(input: $input) }
"#;

const MARK_AS_READ: &str = r#"
mutation MarkRead($input: MarkAsReadInput!) { markAsRead(input: $input) }
"#;

const UPDATE_TOPIC: &str = r#"
mutation Topic($input: UpdateConversationTopicInput!) {
  conversation: updateConversationTopic(input: $input) { id }
}
"#;

struct Fixture {
    ctx: TestCtx,
    schema: AppSchema,
    server_id: String,
    /// Workspace owner.
    alice: Caller,
    bob: Caller,
    /// Workspace member outside the private channel.
    carol: Caller,
}

async fn caller(ctx: &TestCtx, user: &User) -> Caller {
    Caller {
        user_id: user.id,
        session_id: ctx.create_session(user).await,
    }
}

async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let server = ctx.create_server("private-channels-e2e").await;
    let alice_user = ctx.create_user("pc-alice").await;
    let bob_user = ctx.create_user("pc-bob").await;
    let carol_user = ctx.create_user("pc-carol").await;
    ctx.add_owner(&server, &alice_user).await;
    ctx.add_member(&server, &bob_user, ServerMemberRole::Guest)
        .await;
    ctx.add_member(&server, &carol_user, ServerMemberRole::Guest)
        .await;

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    Fixture {
        alice: caller(&ctx, &alice_user).await,
        bob: caller(&ctx, &bob_user).await,
        carol: caller(&ctx, &carol_user).await,
        server_id: server.id.to_string(),
        schema,
        ctx,
    }
}

fn q(query: &str) -> String {
    format!("{query}{CONVERSATION_FIELDS}")
}

impl Fixture {
    /// Users with a stored mention row for `message_id`.
    async fn mentioned_user_ids(&self, message_id: &str) -> Vec<String> {
        let rows = self
            .ctx
            .conn
            .query_all_raw(Statement::from_sql_and_values(
                sea_orm::DbBackend::Postgres,
                "SELECT mentioned_user_id::text AS user_id FROM message_mention \
                 WHERE message_id = $1::uuid",
                [message_id.into()],
            ))
            .await
            .expect("query mentions");
        rows.iter()
            .map(|r| r.try_get::<String>("", "user_id").expect("user_id"))
            .collect()
    }

    async fn create_channel(&self, who: &Caller, title: &str, is_private: bool) -> Value {
        let data = exec_ok(
            &self.schema,
            who.request(
                &q(CREATE_CONVERSATION),
                json!({ "input": {
                    "serverId": self.server_id, "isChannel": true, "title": title,
                    "isPrivate": is_private,
                } }),
            ),
        )
        .await;
        data["conversation"].clone()
    }

    fn ids(&self, conversation_id: &str) -> Value {
        json!({ "serverId": self.server_id, "conversationId": conversation_id })
    }

    async fn conversation(&self, who: &Caller, id: &str) -> Value {
        exec_ok(
            &self.schema,
            who.request(&q(CONVERSATION_QUERY), self.ids(id)),
        )
        .await["conversation"]
            .clone()
    }

    async fn conversation_err(&self, who: &Caller, id: &str) -> String {
        exec_err_code(
            &self.schema,
            who.request(&q(CONVERSATION_QUERY), self.ids(id)),
        )
        .await
    }

    async fn channels(&self, who: &Caller, filter: Value) -> Vec<Value> {
        let data = exec_ok(
            &self.schema,
            who.request(
                &q(CHANNELS_QUERY),
                json!({ "serverId": self.server_id, "filter": filter }),
            ),
        )
        .await;
        data["channels"]["items"]
            .as_array()
            .expect("channel items")
            .clone()
    }

    async fn channel_titles(&self, who: &Caller, filter: Value) -> Vec<String> {
        self.channels(who, filter)
            .await
            .iter()
            .map(|c| c["title"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    async fn send(&self, who: &Caller, id: &str, text: &str) -> Value {
        exec_ok(
            &self.schema,
            who.request(
                SEND_MESSAGE,
                json!({ "input": { "serverId": self.server_id, "conversationId": id, "text": text } }),
            ),
        )
        .await["msg"]
            .clone()
    }

    async fn send_err(&self, who: &Caller, id: &str, text: &str) -> String {
        exec_err_code(
            &self.schema,
            who.request(
                SEND_MESSAGE,
                json!({ "input": { "serverId": self.server_id, "conversationId": id, "text": text } }),
            ),
        )
        .await
    }

    async fn search_ids(&self, who: &Caller, query: &str) -> Vec<String> {
        let data = exec_ok(
            &self.schema,
            who.request(
                SEARCH_QUERY,
                json!({ "input": { "serverId": self.server_id, "query": query } }),
            ),
        )
        .await;
        data["page"]["items"]
            .as_array()
            .expect("search items")
            .iter()
            .map(|m| m["id"].as_str().expect("id").to_string())
            .collect()
    }

    async fn add_members(&self, who: &Caller, id: &str, users: &[&Caller]) -> Value {
        let user_ids: Vec<String> = users.iter().map(|c| c.user_id.to_string()).collect();
        exec_ok(
            &self.schema,
            who.request(
                &q(ADD_MEMBERS),
                json!({ "serverId": self.server_id, "conversationId": id, "userIds": user_ids }),
            ),
        )
        .await["conversation"]
            .clone()
    }
}

fn id_of(conversation: &Value) -> String {
    conversation["id"]
        .as_str()
        .expect("conversation id")
        .to_string()
}

/// A private channel is invisible to a workspace member outside it: `NOT_FOUND` on every read,
/// absent from listings and search, and not joinable. Membership changes (leave, invite) move
/// access with them.
#[tokio::test]
async fn private_channel_is_invisible_to_non_members() {
    let f = setup().await;

    let created = f.create_channel(&f.alice, "hiring", true).await;
    let private_id = id_of(&created);
    assert_eq!(created["isPrivate"], true);
    assert_eq!(created["isMember"], true, "the creator joins automatically");
    assert_eq!(created["createdBy"], f.alice.user_id.to_string());
    assert_eq!(created["memberCount"], 1);

    let with_bob = f.add_members(&f.alice, &private_id, &[&f.bob]).await;
    assert_eq!(with_bob["memberCount"], 2);

    let msg = f.send(&f.alice, &private_id, "zebrafish shortlist").await;
    let msg_id = msg["id"].as_str().expect("message id").to_string();
    exec_ok(
        &f.schema,
        f.alice.request(
            PIN_MESSAGE,
            json!({ "input": { "serverId": f.server_id, "conversationId": private_id, "messageId": msg_id } }),
        ),
    )
    .await;

    // Mentions only reach people who can read the channel: bob yes, carol (outside) no — a
    // mention would otherwise tell her the channel exists.
    let mention = f
        .send(
            &f.alice,
            &private_id,
            &format!(
                "<@{}> and <@{}> please review",
                f.bob.user_id, f.carol.user_id
            ),
        )
        .await;
    let mentioned = f
        .mentioned_user_ids(mention["id"].as_str().expect("id"))
        .await;
    assert_eq!(mentioned, vec![f.bob.user_id.to_string()]);

    // bob (member) reads everything.
    assert_eq!(f.conversation(&f.bob, &private_id).await["isMember"], true);
    assert_eq!(
        f.search_ids(&f.bob, "zebrafish").await,
        vec![msg_id.clone()]
    );

    // carol (workspace member, not in the channel) gets NOT_FOUND everywhere.
    assert_eq!(f.conversation_err(&f.carol, &private_id).await, "NOT_FOUND");
    for query in [MESSAGES_QUERY, PINNED_QUERY] {
        assert_eq!(
            exec_err_code(&f.schema, f.carol.request(query, f.ids(&private_id))).await,
            "NOT_FOUND"
        );
    }
    assert_eq!(f.send_err(&f.carol, &private_id, "hi").await, "NOT_FOUND");
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.carol.request(
                MARK_AS_READ,
                json!({ "input": { "workspaceId": f.server_id, "conversationId": private_id,
                                   "readAt": chrono::Utc::now().to_rfc3339() } }),
            ),
        )
        .await,
        "NOT_FOUND",
        "read state is gated like the messages"
    );
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.carol.request(&q(JOIN_CHANNEL), f.ids(&private_id))
        )
        .await,
        "NOT_FOUND",
        "a private channel cannot be joined"
    );
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.carol.request(
                &q(ADD_MEMBERS),
                json!({ "serverId": f.server_id, "conversationId": private_id,
                        "userIds": [f.carol.user_id.to_string()] }),
            ),
        )
        .await,
        "NOT_FOUND",
        "a non-member cannot invite themselves"
    );
    let browse = json!({ "includeArchived": true });
    assert!(
        !f.channel_titles(&f.carol, browse.clone())
            .await
            .contains(&"hiring".to_string())
    );
    assert!(f.search_ids(&f.carol, "zebrafish").await.is_empty());
    assert!(
        f.search_ids(&f.carol, "in:hiring zebrafish")
            .await
            .is_empty(),
        "`in:` a private channel never leaks"
    );

    // bob leaves and loses access.
    let left = exec_ok(
        &f.schema,
        f.bob.request(LEAVE_CONVERSATION, f.ids(&private_id)),
    )
    .await;
    assert_eq!(left["leaveConversation"], true);
    assert_eq!(f.conversation_err(&f.bob, &private_id).await, "NOT_FOUND");
    assert!(f.search_ids(&f.bob, "zebrafish").await.is_empty());

    // alice invites carol: she gains access.
    let with_carol = f.add_members(&f.alice, &private_id, &[&f.carol]).await;
    assert_eq!(with_carol["memberCount"], 2);
    let seen = f.conversation(&f.carol, &private_id).await;
    assert_eq!(seen["isMember"], true);
    assert!(
        f.channel_titles(&f.carol, json!({ "joinedOnly": true }))
            .await
            .contains(&"hiring".to_string())
    );
    let pinned = exec_ok(&f.schema, f.carol.request(PINNED_QUERY, f.ids(&private_id))).await;
    assert_eq!(pinned["pinnedMessages"][0]["id"], msg_id);
    assert_eq!(f.search_ids(&f.carol, "zebrafish").await, vec![msg_id]);
}

/// Public channels appear in browse for everyone, can be read without joining, and are joinable;
/// `isMember` and `joinedOnly` track membership.
#[tokio::test]
async fn public_channel_join_and_leave() {
    let f = setup().await;
    let created = f.create_channel(&f.alice, "launch", false).await;
    let public_id = id_of(&created);
    assert_eq!(created["isPrivate"], false);

    // Browse shows it; carol has not joined; reading works without joining.
    let browse = f.channels(&f.carol, Value::Null).await;
    let launch = browse
        .iter()
        .find(|c| c["id"] == public_id.as_str())
        .expect("public channel in browse");
    assert_eq!(launch["isMember"], false);
    assert!(
        !f.channel_titles(&f.carol, json!({ "joinedOnly": true }))
            .await
            .contains(&"launch".to_string())
    );
    exec_ok(
        &f.schema,
        f.carol.request(MESSAGES_QUERY, f.ids(&public_id)),
    )
    .await;

    // Join (idempotent).
    for _ in 0..2 {
        let joined = exec_ok(
            &f.schema,
            f.carol.request(&q(JOIN_CHANNEL), f.ids(&public_id)),
        )
        .await;
        assert_eq!(joined["conversation"]["isMember"], true);
    }
    let joined_list = f.channels(&f.carol, json!({ "joinedOnly": true })).await;
    let launch = joined_list
        .iter()
        .find(|c| c["id"] == public_id.as_str())
        .expect("joined channel listed");
    assert_eq!(launch["isMember"], true);
    assert_eq!(launch["memberCount"], 2);

    // A guest may not remove someone else; they may remove themselves.
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.carol.request(
                REMOVE_MEMBER,
                json!({ "serverId": f.server_id, "conversationId": public_id,
                        "userId": f.alice.user_id.to_string() }),
            ),
        )
        .await,
        "FORBIDDEN"
    );
    let removed = exec_ok(
        &f.schema,
        f.carol.request(
            REMOVE_MEMBER,
            json!({ "serverId": f.server_id, "conversationId": public_id,
                    "userId": f.carol.user_id.to_string() }),
        ),
    )
    .await;
    assert_eq!(removed["removeConversationMember"], true);
    assert_eq!(
        f.conversation(&f.carol, &public_id).await["isMember"],
        false
    );

    // DMs cannot be left; private flag is rejected for non-channels.
    let dm = exec_ok(
        &f.schema,
        f.alice.request(
            &q(CREATE_CONVERSATION),
            json!({ "input": { "serverId": f.server_id, "otherUserId": f.bob.user_id.to_string() } }),
        ),
    )
    .await;
    let dm_id = id_of(&dm["conversation"]);
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.alice.request(LEAVE_CONVERSATION, f.ids(&dm_id))
        )
        .await,
        "VALIDATION"
    );
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.alice.request(
                &q(CREATE_CONVERSATION),
                json!({ "input": { "serverId": f.server_id,
                                   "memberIds": [f.bob.user_id.to_string()], "isPrivate": true } }),
            ),
        )
        .await,
        "VALIDATION"
    );
}

/// Archiving is Owner/Moderator only, makes the channel read-only for every write path tested
/// here, and hides it from the default list but not from `includeArchived`.
#[tokio::test]
async fn archived_channel_is_read_only_and_hidden_by_default() {
    let f = setup().await;
    let created = f.create_channel(&f.alice, "retro", false).await;
    let id = id_of(&created);
    let before = f.send(&f.alice, &id, "last words").await;
    let before_id = before["id"].as_str().expect("id").to_string();

    // A guest gets FORBIDDEN (role failure).
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.bob.request(
                &q(SET_ARCHIVED),
                json!({ "serverId": f.server_id, "conversationId": id, "archived": true }),
            ),
        )
        .await,
        "FORBIDDEN"
    );

    let archived = exec_ok(
        &f.schema,
        f.alice.request(
            &q(SET_ARCHIVED),
            json!({ "serverId": f.server_id, "conversationId": id, "archived": true }),
        ),
    )
    .await;
    assert_eq!(archived["conversation"]["isArchived"], true);
    assert!(archived["conversation"]["archivedAt"].is_string());

    // Writes fail with CONVERSATION_ARCHIVED.
    assert_eq!(
        f.send_err(&f.alice, &id, "anyone?").await,
        "CONVERSATION_ARCHIVED"
    );
    assert_eq!(
        f.send_err(&f.bob, &id, "anyone?").await,
        "CONVERSATION_ARCHIVED"
    );
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.alice.request(
                PIN_MESSAGE,
                json!({ "input": { "serverId": f.server_id, "conversationId": id, "messageId": before_id } }),
            ),
        )
        .await,
        "CONVERSATION_ARCHIVED"
    );
    assert_eq!(
        exec_err_code(
            &f.schema,
            f.alice.request(
                UPDATE_TOPIC,
                json!({ "input": { "serverId": f.server_id, "conversationId": id, "topic": "x" } }),
            ),
        )
        .await,
        "CONVERSATION_ARCHIVED"
    );
    assert_eq!(
        exec_err_code(&f.schema, f.carol.request(&q(JOIN_CHANNEL), f.ids(&id))).await,
        "CONVERSATION_ARCHIVED"
    );

    // Reads still work, including search.
    let page = exec_ok(&f.schema, f.bob.request(MESSAGES_QUERY, f.ids(&id))).await;
    assert_eq!(page["page"]["items"][0]["id"], before_id);
    assert_eq!(f.search_ids(&f.bob, "words").await, vec![before_id]);

    // Hidden from the default list, present with includeArchived.
    assert!(
        !f.channel_titles(&f.bob, Value::Null)
            .await
            .contains(&"retro".to_string())
    );
    assert!(
        f.channel_titles(&f.bob, json!({ "includeArchived": true }))
            .await
            .contains(&"retro".to_string())
    );

    // Unarchive restores writes.
    let restored = exec_ok(
        &f.schema,
        f.alice.request(
            &q(SET_ARCHIVED),
            json!({ "serverId": f.server_id, "conversationId": id, "archived": false }),
        ),
    )
    .await;
    assert_eq!(restored["conversation"]["isArchived"], false);
    f.send(&f.bob, &id, "back again").await;
    assert!(
        f.channel_titles(&f.bob, Value::Null)
            .await
            .contains(&"retro".to_string())
    );
}

/// The workspace owner does not see a private channel they are not in, but can still archive it
/// by id (the documented recovery path for an orphaned private channel).
#[tokio::test]
async fn owner_archives_private_channel_they_are_not_in() {
    let f = setup().await;
    let created = f.create_channel(&f.bob, "bob-only", true).await;
    let id = id_of(&created);

    assert_eq!(f.conversation_err(&f.alice, &id).await, "NOT_FOUND");
    let archived = exec_ok(
        &f.schema,
        f.alice.request(
            &q(SET_ARCHIVED),
            json!({ "serverId": f.server_id, "conversationId": id, "archived": true }),
        ),
    )
    .await;
    assert_eq!(archived["conversation"]["isArchived"], true);
    assert_eq!(
        f.send_err(&f.bob, &id, "hello?").await,
        "CONVERSATION_ARCHIVED"
    );
}
