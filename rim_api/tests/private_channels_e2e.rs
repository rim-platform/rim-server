//! Through-the-stack e2e tests for private channels, channel join/leave/invite, and
//! archiving.
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
use rim_services::auth::dto::RegistrationMode;

use support::graphql::{
    Caller, anonymous_request, assert_each_refused, assert_each_refused_as, exec_err_code, exec_ok,
    merged,
};
use support::roots::{
    ATTACHMENT_ROOTS, CONVERSATION_MESSAGE_ROOTS, CONVERSATION_NOTIFICATION_ROOTS,
    CONVERSATION_ROOTS, JOIN_ROOTS, MESSAGE_ROOTS, Targets,
};
use support::{TestCtx, unknown_id};

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

const EDIT_MESSAGE: &str = r#"
mutation Edit($input: EditMessageInput!) { editMessage(input: $input) { id } }"#;

const PIN_MESSAGE: &str = r#"
mutation Pin($input: PinMessageInput!) { pinMessage(input: $input) }
"#;

const UNREAD_COUNTS: &str = r#"
query Unread($workspaceId: ID!) { unreadCounts(workspaceId: $workspaceId) { conversationId count } }
"#;

const CREATE_INVITE: &str = r#"
mutation CreateInvite($input: CreateInviteInput!) { invite: createInvite(input: $input) { inviteToken } }
"#;

const JOIN_BY_INVITE: &str = r#"
mutation JoinByInvite($input: JoinServerByInviteInput!) {
  joinServerByInvite(input: $input) { serverId }
}
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
    server: rim_core::server::model::Server,
    /// Workspace owner.
    alice: Caller,
    bob: Caller,
    /// Workspace member outside the private channel.
    carol: Caller,
}

async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let server = ctx.create_server("private-channels-e2e").await;
    let alice_user = ctx.create_user("pc-alice").await;
    let bob_user = ctx.create_user("pc-bob").await;
    let carol_user = ctx.create_user("pc-carol").await;
    ctx.add_owner(&server, &alice_user).await;
    ctx.add_member(&server, &bob_user, ServerMemberRole::Member)
        .await;
    ctx.add_member(&server, &carol_user, ServerMemberRole::Member)
        .await;

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    Fixture {
        alice: ctx.caller(&alice_user).await,
        bob: ctx.caller(&bob_user).await,
        carol: ctx.caller(&carol_user).await,
        server_id: server.id.to_string(),
        server,
        schema,
        ctx,
    }
}

fn q(query: &str) -> String {
    format!("{query}{CONVERSATION_FIELDS}")
}

impl Fixture {
    /// Conversation ids with an unread-count row for `who`.
    async fn unread_ids(&self, who: &Caller) -> Vec<String> {
        let data = exec_ok(
            &self.schema,
            who.request(UNREAD_COUNTS, json!({ "workspaceId": self.server_id })),
        )
        .await;
        data["unreadCounts"]
            .as_array()
            .expect("unread counts")
            .iter()
            .map(|c| c["conversationId"].as_str().expect("id").to_string())
            .collect()
    }

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

/// In a public channel a mention reaches any workspace member but a guest, joined or not; the
/// author and unknown ids are dropped, and an edit replaces the message's mentions.
#[tokio::test]
async fn public_channel_mentions_reach_workspace_members_but_not_outside_guests() {
    let f = setup().await;
    let guest = f.ctx.create_user("pc-guest").await;
    f.ctx
        .add_member(&f.server, &guest, ServerMemberRole::Guest)
        .await;

    let lobby = id_of(&f.create_channel(&f.alice, "lobby", false).await);
    let msg = f
        .send(
            &f.alice,
            &lobby,
            &format!(
                "<@{}> <@{}> <@{}> <@{}> look",
                f.alice.user_id,
                f.bob.user_id,
                guest.id,
                uuid::Uuid::new_v4()
            ),
        )
        .await;
    let msg_id = msg["id"].as_str().expect("id").to_string();
    assert_eq!(
        f.mentioned_user_ids(&msg_id).await,
        vec![f.bob.user_id.to_string()]
    );

    exec_ok(
        &f.schema,
        f.alice.request(
            EDIT_MESSAGE,
            json!({ "input": {
                "serverId": f.server_id, "conversationId": lobby, "messageId": msg_id,
                "text": format!("<@{}> instead", f.carol.user_id),
            } }),
        ),
    )
    .await;
    assert_eq!(
        f.mentioned_user_ids(&msg_id).await,
        vec![f.carol.user_id.to_string()]
    );
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

    // carol (workspace member, not in the channel) gets NOT_FOUND from every root that names
    // the channel, its messages, its thread or its attachment, inviting herself included.
    let (alice, bob, carol) = (&f.alice, &f.bob, &f.carol);
    let t = Targets::seed(
        &f.schema,
        &f.server_id,
        &private_id,
        alice,
        bob,
        carol,
        "agenda",
    )
    .await;
    let roots = [
        CONVERSATION_MESSAGE_ROOTS,
        MESSAGE_ROOTS,
        ATTACHMENT_ROOTS,
        CONVERSATION_ROOTS,
        JOIN_ROOTS,
        CONVERSATION_NOTIFICATION_ROOTS,
    ];
    assert_each_refused_as(&f.schema, carol, &t.calls(&roots), "NOT_FOUND").await;
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

    // Unread counts only cover conversations carol is a member of: an unjoined public channel
    // with new messages carries no badge.
    f.send(&f.alice, &public_id, "launch day").await;
    assert!(!f.unread_ids(&f.carol).await.contains(&public_id));

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
    // Unread counts start at the join: the earlier message does not count, a new one does.
    assert!(!f.unread_ids(&f.carol).await.contains(&public_id));
    f.send(&f.alice, &public_id, "launch is live").await;
    assert!(
        f.unread_ids(&f.carol).await.contains(&public_id),
        "a joined channel counts unread messages"
    );

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

/// A user who joins the workspace after the channel back-fill is auto-joined to every public,
/// unarchived channel, and to nothing private or archived.
#[tokio::test]
async fn new_workspace_member_auto_joins_public_channels() {
    let f = setup().await;
    let public_id = id_of(&f.create_channel(&f.alice, "lobby", false).await);
    let private_id = id_of(&f.create_channel(&f.alice, "board", true).await);
    let archived_id = id_of(&f.create_channel(&f.alice, "old-news", false).await);
    exec_ok(
        &f.schema,
        f.alice.request(
            &q(SET_ARCHIVED),
            json!({ "serverId": f.server_id, "conversationId": archived_id, "archived": true }),
        ),
    )
    .await;

    let invite = exec_ok(
        &f.schema,
        f.alice.request(
            CREATE_INVITE,
            json!({ "input": { "serverId": f.server_id } }),
        ),
    )
    .await;
    let token = invite["invite"]["inviteToken"]
        .as_str()
        .expect("invite token");

    let dave_user = f.ctx.create_user("pc-dave").await;
    let dave = f.ctx.caller(&dave_user).await;
    exec_ok(
        &f.schema,
        dave.request(JOIN_BY_INVITE, json!({ "input": { "inviteToken": token } })),
    )
    .await;

    let joined = f
        .channels(
            &dave,
            json!({ "joinedOnly": true, "includeArchived": true }),
        )
        .await;
    let lobby = joined
        .iter()
        .find(|c| c["id"] == public_id.as_str())
        .expect("public channel joined automatically");
    assert_eq!(lobby["isMember"], true);
    assert!(
        joined.iter().all(|c| c["id"] != archived_id.as_str()),
        "archived channels are not auto-joined"
    );

    let browse = f.channels(&dave, json!({ "includeArchived": true })).await;
    assert!(
        browse.iter().all(|c| c["id"] != private_id.as_str()),
        "the private channel stays invisible"
    );
    assert_eq!(f.conversation_err(&dave, &private_id).await, "NOT_FOUND");
}

// ── Every conversation root ───────────────────────────────────────────────

/// Roots over workspace `$s` as a whole.
const WORKSPACE_ROOTS: &[&str] = &[
    "query($s: ID!) { channels(serverId: $s, dir: AFTER, limit: 5) { items { id } } }",
    "query($s: ID!) { inboxConversations(serverId: $s, dir: AFTER, limit: 5) { items { id } } }",
    "query($s: ID!) { unreadCounts(workspaceId: $s) { count } }",
    "mutation($s: ID!) { createConversation(input: { serverId: $s, isChannel: true, \
     title: \"new\" }) { id } }",
    "mutation($s: ID!) { markAllAsRead(input: { workspaceId: $s }) }",
];

/// Roots only an Owner or Moderator may call on channel `$c`.
const MODERATOR_ROOTS: &[&str] = &["mutation($s: ID!, $c: ID!) { setConversationArchived(\
     serverId: $s, conversationId: $c, archived: true) { id } }"];

const MY_SERVERS: &str = "query { myServers { id name owner { username } } }";

impl Fixture {
    /// Channel `conversation_id` of the workspace, with `user` as the member the roots remove
    /// or add.
    fn targets(&self, conversation_id: &str, user: &Caller) -> Targets {
        Targets::conversation(&self.server_id, conversation_id, user)
    }
}

/// Every conversation and workspace root refuses a caller without a session and one outside
/// the workspace, who changes nothing; `myServers` lists only the caller's own workspaces, each
/// with its owner.
#[tokio::test]
async fn every_conversation_root_refuses_callers_outside_the_workspace() {
    let f = setup().await;
    let id = id_of(&f.create_channel(&f.alice, "lobby", false).await);
    let roots = [
        CONVERSATION_ROOTS,
        JOIN_ROOTS,
        MODERATOR_ROOTS,
        WORKSPACE_ROOTS,
    ];
    let calls = f.targets(&id, &f.bob).calls(&roots);
    let mut anonymous = calls.clone();
    anonymous.push((MY_SERVERS, json!({})));
    assert_each_refused(&f.schema, &anonymous, "UNAUTHENTICATED", anonymous_request).await;
    let olive = f.ctx.outsider("pc-olive").await;
    assert_each_refused_as(&f.schema, &olive, &calls, "FORBIDDEN").await;

    let lobby = f.conversation(&f.alice, &id).await;
    assert_eq!(
        (&lobby["isArchived"], &lobby["memberCount"]),
        (&json!(false), &json!(1))
    );
    let servers = |who: &Caller| exec_ok(&f.schema, who.request(MY_SERVERS, json!({})));
    assert_eq!(servers(&olive).await["myServers"], json!([]));
    assert_eq!(
        servers(&f.carol).await["myServers"],
        json!([{
            "id": f.server_id, "name": "private-channels-e2e", "owner": { "username": "pc-alice" },
        }])
    );
}

/// A guest nobody added to a public channel gets `NOT_FOUND` from every root that needs to see
/// it, removing someone else included. A guest opens direct messages but not groups. A guest
/// sees the workspace's owner without sharing a conversation with them.
#[tokio::test]
async fn a_guest_does_not_see_a_public_channel_nobody_added_them_to() {
    let f = setup().await;
    let id = id_of(&f.create_channel(&f.alice, "lobby", false).await);
    let gina = f
        .ctx
        .newcomer(&f.server_id, "pc-gina", ServerMemberRole::Guest)
        .await;
    let calls = f.targets(&id, &f.alice).calls(&[CONVERSATION_ROOTS]);
    assert_each_refused_as(&f.schema, &gina, &calls, "NOT_FOUND").await;
    let servers = exec_ok(&f.schema, gina.request(MY_SERVERS, json!({}))).await;
    assert_eq!(
        servers["myServers"][0]["owner"],
        json!({ "username": "pc-alice" })
    );

    let with = |participants: Value| {
        let input = json!({ "serverId": f.server_id });
        gina.request(
            &q(CREATE_CONVERSATION),
            json!({ "input": merged(input, participants) }),
        )
    };
    let bob = f.bob.user_id.to_string();
    let group = with(json!({ "memberIds": [bob, f.carol.user_id.to_string()] }));
    assert_eq!(exec_err_code(&f.schema, group).await, "FORBIDDEN");
    let dm = exec_ok(&f.schema, with(json!({ "otherUserId": bob }))).await;
    assert_eq!(dm["conversation"]["conversationType"], "DIRECT");
}

/// An id that names nothing is `NOT_FOUND`; a malformed one is `VALIDATION`.
#[tokio::test]
async fn every_conversation_root_answers_unknown_and_malformed_ids() {
    let f = setup().await;
    let roots = [CONVERSATION_ROOTS, JOIN_ROOTS, MODERATOR_ROOTS];
    let nowhere = f.targets(&unknown_id(), &f.bob).calls(&roots);
    assert_each_refused_as(&f.schema, &f.alice, &nowhere, "NOT_FOUND").await;

    let mut malformed = f.targets("not-an-id", &f.bob);
    malformed.user = "not-an-id".to_string();
    let mut calls = malformed.calls(&roots);
    malformed.server = "not-an-id".to_string();
    calls.extend(malformed.calls(&[WORKSPACE_ROOTS]));
    assert_each_refused_as(&f.schema, &f.alice, &calls, "VALIDATION").await;
}

/// `pinMessage` and `unpinMessage` reach only a message of the conversation they name: a
/// member of a public channel cannot touch the pins of a private one through it.
#[tokio::test]
async fn pins_reach_only_messages_of_the_named_conversation() {
    let f = setup().await;
    let board = id_of(&f.create_channel(&f.alice, "board", true).await);
    let lobby = id_of(&f.create_channel(&f.alice, "lobby", false).await);
    let msg = f.send(&f.alice, &board, "the plan").await;
    let msg_id = msg["id"].as_str().expect("id").to_string();
    let pin = |conversation_id: &str| {
        json!({ "input": {
            "serverId": f.server_id, "conversationId": conversation_id, "messageId": msg_id,
        } })
    };
    exec_ok(&f.schema, f.alice.request(PIN_MESSAGE, pin(&board))).await;

    let unpin = "mutation Unpin($input: UnpinMessageInput!) { unpinMessage(input: $input) }";
    for doc in [PIN_MESSAGE, unpin] {
        let code = exec_err_code(&f.schema, f.carol.request(doc, pin(&lobby))).await;
        assert_eq!(code, "NOT_FOUND", "{doc}");
    }
    let pinned = exec_ok(&f.schema, f.alice.request(PINNED_QUERY, f.ids(&board))).await;
    assert_eq!(pinned["pinnedMessages"], json!([{ "id": msg_id }]));
}

/// Inputs a conversation root refuses: a title over 100 characters, a topic over 512, an empty
/// invitation, inviting or messaging someone outside the workspace, and a DM with oneself.
#[tokio::test]
async fn conversation_inputs_are_bounded() {
    let f = setup().await;
    let id = id_of(&f.create_channel(&f.alice, "lobby", false).await);
    let olive = f.ctx.outsider("pc-olive").await;
    let (olive_id, alice_id) = (olive.user_id.to_string(), f.alice.user_id.to_string());
    let create = |input: Value| {
        let input = merged(json!({ "serverId": f.server_id }), input);
        (q(CREATE_CONVERSATION), json!({ "input": input }))
    };
    let invite = |user_ids: Value| {
        let vars = json!({ "serverId": f.server_id, "conversationId": id, "userIds": user_ids });
        (q(ADD_MEMBERS), vars)
    };
    let topic = json!({ "input": {
        "serverId": f.server_id, "conversationId": id, "topic": "t".repeat(513),
    } });
    let refused = [
        create(json!({ "isChannel": true, "title": "t".repeat(101) })),
        (UPDATE_TOPIC.to_string(), topic),
        invite(json!([])),
        invite(json!([olive_id])),
        create(json!({ "otherUserId": alice_id })),
    ];
    for (doc, vars) in refused {
        let code = exec_err_code(&f.schema, f.alice.request(&doc, vars.clone())).await;
        assert_eq!(code, "VALIDATION", "{vars}");
    }
    // Naming someone outside the workspace for a new DM is answered as a refusal of the caller.
    let dm = create(json!({ "otherUserId": olive_id }));
    let code = exec_err_code(&f.schema, f.alice.request(&dm.0, dm.1)).await;
    assert_eq!(code, "FORBIDDEN");
}
