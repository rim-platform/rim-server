//! Self-service account deletion over a real database: the request and its refusals, the cancel,
//! and the purge run through the job's entry point once the grace period ended.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests --test account_deletion_e2e -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};

use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;
use rim_infra_services::scheduled_purges::purge_deleted_accounts;
use rim_services::auth::dto::RegistrationMode;

use support::TestCtx;
use support::graphql::{Caller, anonymous_request, exec_err, exec_err_code, exec_ok};
use support::identity::{SIGNUP_WITH_KEY, signup_input};
use support::roles::create_channel;
use support::sessions::{LOGIN, PASSWORD, login, with_session};

const REQUEST: &str = r#"mutation($u: String!) {
  requestAccountDeletion(input: { confirmUsername: $u }) { scheduledFor }
}"#;

const CANCEL: &str = "mutation { cancelAccountDeletion }";

const ME: &str = "query { me { username deletionScheduledFor } }";

const TRANSFER: &str = r#"mutation($s: ID!, $u: ID!) {
  transferWorkspaceOwnership(input: { serverId: $s, userId: $u }) { id }
}"#;

const SEND: &str = r#"mutation($s: ID!, $c: ID!, $text: String!) {
  sendMessage(input: { serverId: $s, conversationId: $c, text: $text }) { id }
}"#;

const REPLY: &str = r#"mutation($s: ID!, $c: ID!, $m: ID!, $text: String!) {
  replyToMessage(input: { serverId: $s, conversationId: $c, rootMessageId: $m, text: $text }) {
    id threadId
  }
}"#;

const JOIN: &str =
    "mutation($s: ID!, $c: ID!) { joinChannel(serverId: $s, conversationId: $c) { id } }";

const MESSAGES: &str = r#"query($s: ID!, $c: ID!) {
  messages(serverId: $s, conversationId: $c, dir: AFTER, limit: 50, includeDeleted: true) {
    items { id text isDeleted rootThreadId author { username displayName } }
  }
}"#;

const THREAD: &str = r#"query($s: ID!, $c: ID!, $t: ID!) {
  threadMessages(serverId: $s, conversationId: $c, threadId: $t, dir: AFTER, limit: 50,
                 includeDeleted: true) { items { id text } }
}"#;

const SEARCH: &str = r#"query($s: ID!, $q: String!) {
  searchMessages(input: { serverId: $s, query: $q }) { items { id } }
}"#;

const MEMBERS: &str = "query($s: ID!) { serverMembers(serverId: $s) { userId memberRole } }";

const SET_AVATAR: &str =
    "mutation($u: String!) { updateMyProfile(input: { avatarUrl: $u }) { avatarUrl } }";

/// Where the test provider's local storage keeps uploaded files.
const STORAGE_ROOT: &str = "/tmp/rim_api_e2e_test_storage";

const GROUP: &str = r#"query($s: ID!, $c: ID!) {
  conversation(serverId: $s, conversationId: $c) {
    mlsGroup { pendingRemoves { principalId deviceId } }
  }
}"#;

fn request_as(who: &Caller, username: &str) -> async_graphql::Request {
    who.request(REQUEST, json!({ "u": username }))
}

fn scheduled_for(data: &Value) -> DateTime<Utc> {
    data["requestAccountDeletion"]["scheduledFor"]
        .as_str()
        .expect("scheduledFor")
        .parse()
        .expect("RFC 3339")
}

/// The directory holding `user_id`'s avatar files.
fn avatar_dir(user_id: UserId) -> PathBuf {
    PathBuf::from(STORAGE_ROOT).join(format!("avatars/{user_id}"))
}

/// Store a file named each of `names` in `user_id`'s avatar directory.
fn store_avatars(user_id: UserId, names: &[&str]) {
    let dir = avatar_dir(user_id);
    std::fs::create_dir_all(&dir).expect("avatar dir");
    for name in names {
        std::fs::write(dir.join(name), b"x").expect("avatar file");
    }
}

/// The names of the files in `user_id`'s avatar directory, sorted.
fn avatar_files(user_id: UserId) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(avatar_dir(user_id)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|e| e.expect("entry").file_name().into_string().expect("utf-8"))
        .collect();
    names.sort();
    names
}

/// The first row's first column of `sql`, as text.
async fn scalar(ctx: &TestCtx, sql: &str) -> Option<String> {
    ctx.conn
        .query_one_raw(Statement::from_string(DbBackend::Postgres, sql.to_string()))
        .await
        .expect(sql)
        .map(|row| row.try_get_by_index::<String>(0).expect("text"))
}

/// The request is refused for a mistyped username and while the account owns a workspace with
/// other members; once it has handed the workspace over it is accepted, ends the other sessions,
/// shows on `me`, refuses a second request and can be cancelled.
#[tokio::test]
async fn a_deletion_is_requested_with_the_username_shows_on_me_and_can_be_cancelled() {
    let ctx = TestCtx::new().await;
    let alice = ctx.create_password_user("alice").await;
    let bob = ctx.create_user("bob").await;
    let team = ctx.create_server("Team").await;
    ctx.add_owner(&team, &alice).await;
    ctx.add_member(&team, &bob, ServerMemberRole::Member).await;
    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let provider = ctx.build_provider();
    let caller = ctx.caller(&alice).await;
    let laptop = login(&schema, "alice").await;

    assert_eq!(
        exec_err_code(&schema, request_as(&caller, "alicia")).await,
        "VALIDATION"
    );
    let (code, message) = exec_err(&schema, request_as(&caller, "alice")).await;
    assert_eq!(code, "VALIDATION");
    assert!(
        message.contains("transfer or delete") && message.contains("Team"),
        "{message}"
    );

    exec_ok(
        &schema,
        caller.request(
            TRANSFER,
            json!({ "s": team.id.to_string(), "u": bob.id.to_string() }),
        ),
    )
    .await;
    let before = Utc::now();
    let data = exec_ok(&schema, request_as(&caller, "alice")).await;
    let at = scheduled_for(&data);
    assert!(at >= before + Duration::days(14) && at <= Utc::now() + Duration::days(14));

    let me = exec_ok(&schema, caller.request(ME, json!({}))).await;
    let shown: DateTime<Utc> = me["me"]["deletionScheduledFor"]
        .as_str()
        .expect("pending")
        .parse()
        .expect("RFC 3339");
    assert_eq!(shown, at);
    assert_eq!(
        exec_err_code(&schema, with_session(&provider, &laptop, ME).await).await,
        "UNAUTHENTICATED"
    );
    assert_eq!(
        exec_err_code(&schema, request_as(&caller, "alice")).await,
        "CONFLICT"
    );

    let data = exec_ok(&schema, caller.request(CANCEL, json!({}))).await;
    assert_eq!(data["cancelAccountDeletion"], json!(true));
    let me = exec_ok(&schema, caller.request(ME, json!({}))).await;
    assert_eq!(me["me"]["deletionScheduledFor"], Value::Null);
    let data = exec_ok(&schema, caller.request(CANCEL, json!({}))).await;
    assert_eq!(data["cancelAccountDeletion"], json!(false));
}

/// With no grace period the purge job takes the account at its next run: the account cannot
/// sign in and its username is free, its plaintext messages are gone or, where someone replied,
/// shown as deleted, its encrypted message stays with the tombstone as author, and it is in no
/// member list or search result. The workspace it was alone in is deleted; the one it handed
/// over stays with its new owner.
#[tokio::test]
async fn the_purge_leaves_a_tombstone_that_cannot_sign_in_and_shows_nowhere() {
    let ctx = TestCtx::new().await;
    let (schema, provider) =
        ctx.build_with_deletion_grace(Duration::zero(), RegistrationMode::InviteOnly);
    let alice_user = ctx.create_password_user("alice").await;
    let bob_user = ctx.create_user("bob").await;
    let shared = ctx.create_server("Shared").await;
    ctx.add_owner(&shared, &alice_user).await;
    ctx.add_member(&shared, &bob_user, ServerMemberRole::Member)
        .await;
    let alone = ctx.create_server("Alone").await;
    ctx.add_owner(&alone, &alice_user).await;
    let (alice, bob) = (ctx.caller(&alice_user).await, ctx.caller(&bob_user).await);
    let session = login(&schema, "alice").await;
    let s = shared.id.to_string();
    let c = create_channel(&schema, &bob, &s, "general", false).await;
    exec_ok(&schema, alice.request(JOIN, json!({ "s": s, "c": c }))).await;
    let send = |who: Caller, text: &str| {
        let request = who.request(SEND, json!({ "s": s, "c": c, "text": text }));
        let schema = &schema;
        async move {
            exec_ok(schema, request).await["sendMessage"]["id"]
                .as_str()
                .expect("id")
                .to_string()
        }
    };
    let plan = send(alice, "the zanzibar plan").await;
    let question = send(alice, "a question").await;
    let reply = exec_ok(
        &schema,
        bob.request(
            REPLY,
            json!({ "s": s, "c": c, "m": question, "text": "bob answers" }),
        ),
    )
    .await;
    let thread_id = reply["replyToMessage"]["threadId"]
        .as_str()
        .expect("thread")
        .to_string();
    let encrypted = uuid::Uuid::now_v7();
    ctx.conn
        .execute_unprepared(&format!(
            "INSERT INTO message (id, conversation_id, sent_by, encrypted, mls_ciphertext, \
             mls_incarnation, mls_epoch) VALUES ('{encrypted}', '{c}', '{}', true, '\\x0102', 0, 1)",
            alice.user_id
        ))
        .await
        .expect("encrypted message");
    let found = exec_ok(
        &schema,
        bob.request(SEARCH, json!({ "s": s, "q": "zanzibar" })),
    )
    .await;
    assert_eq!(
        found["searchMessages"]["items"].as_array().map(Vec::len),
        Some(1)
    );

    exec_ok(
        &schema,
        alice.request(TRANSFER, json!({ "s": s, "u": bob.user_id.to_string() })),
    )
    .await;
    exec_ok(&schema, request_as(&alice, "alice")).await;
    let run = purge_deleted_accounts(provider.account_deletion_service().as_ref())
        .await
        .expect("run");
    assert_eq!(run.failed, vec![]);
    assert_eq!(run.purged.len(), 1);
    assert_eq!(run.purged[0].user_id, alice.user_id);

    // No sign-in, no session, and the username is free.
    let refused = anonymous_request(
        LOGIN,
        json!({ "input": { "username": "alice", "password": PASSWORD } }),
    );
    assert_eq!(exec_err_code(&schema, refused).await, "UNAUTHENTICATED");
    assert_eq!(
        exec_err_code(&schema, with_session(&provider, &session, ME).await).await,
        "UNAUTHENTICATED"
    );
    ctx.create_password_user("alice").await;

    // Bob sees the question as deleted, by "Deleted account", with his reply under it; the
    // plan is gone.
    let page = exec_ok(&schema, bob.request(MESSAGES, json!({ "s": s, "c": c }))).await;
    let items = page["messages"]["items"].as_array().expect("items");
    assert!(items.iter().all(|m| m["id"] != json!(plan)));
    let shown = items
        .iter()
        .find(|m| m["id"] == json!(question))
        .expect("the question stays");
    assert_eq!(shown["isDeleted"], json!(true));
    assert_eq!(shown["text"], Value::Null);
    assert_eq!(shown["rootThreadId"], json!(thread_id));
    assert_eq!(shown["author"]["displayName"], json!("Deleted account"));
    let thread = exec_ok(
        &schema,
        bob.request(THREAD, json!({ "s": s, "c": c, "t": thread_id })),
    )
    .await;
    assert_eq!(
        thread["threadMessages"]["items"][0]["text"],
        json!("bob answers")
    );

    // The ciphertext stays, written by the tombstone.
    let author = scalar(
        &ctx,
        &format!(
            "SELECT u.username FROM message m JOIN \"user\" u ON u.id = m.sent_by \
             WHERE m.id = '{encrypted}' AND m.mls_ciphertext IS NOT NULL"
        ),
    )
    .await
    .expect("the encrypted message stays");
    let prefix = &alice.user_id.to_string().replace('-', "")[..8];
    assert_eq!(author, format!("deleted-{prefix}"));

    // In no member list or search result, and alone in no workspace any more.
    let members = exec_ok(&schema, bob.request(MEMBERS, json!({ "s": s }))).await;
    let members = members["serverMembers"].as_array().expect("members");
    assert_eq!(
        members,
        &vec![json!({ "userId": bob.user_id.to_string(), "memberRole": "OWNER" })]
    );
    let found = exec_ok(
        &schema,
        bob.request(SEARCH, json!({ "s": s, "q": "zanzibar" })),
    )
    .await;
    assert_eq!(found["searchMessages"]["items"], json!([]));
    assert_eq!(
        scalar(
            &ctx,
            &format!("SELECT name FROM server WHERE id = '{}'", alone.id)
        )
        .await,
        None
    );
}

/// An account with a key identity: after the purge its revoked device is a pending remove of
/// the encrypted group it was in, so the other members' clients take it out, and its key
/// identity cannot sign up here again.
#[tokio::test]
async fn a_purged_key_identity_leaves_its_groups_and_cannot_sign_up_again() {
    let ctx = TestCtx::new().await;
    let (schema, provider) =
        ctx.build_with_deletion_grace(Duration::zero(), RegistrationMode::Open);
    let alice = ctx.enrolled(&schema, "alice", 41).await;
    let bob = ctx.enrolled(&schema, "bob", 42).await;
    let shared = ctx.create_server("Shared").await;
    ctx.add_owner(&shared, &bob.user).await;
    ctx.add_member(&shared, &alice.user, ServerMemberRole::Member)
        .await;
    // An encrypted group both devices are leaves of, as a group creation leaves it.
    let (s, c) = (shared.id.to_string(), uuid::Uuid::now_v7());
    ctx.conn
        .execute_unprepared(&format!(
            "INSERT INTO conversation (id, conversation_type, server_id, encrypted_at) \
             VALUES ('{c}', 'group', '{s}', now()); \
             INSERT INTO conversation_member (id, user_id, conversation_id) VALUES \
             (gen_random_uuid(), '{a}', '{c}'), (gen_random_uuid(), '{b}', '{c}'); \
             INSERT INTO mls_group (conversation_id, incarnation, group_id, cipher_suite, epoch) \
             VALUES ('{c}', 1, '\\x01', 79, 1); \
             UPDATE device_key SET last_key_package_upload_at = now(); \
             INSERT INTO mls_group_device (conversation_id, incarnation, device_key_id, user_id, \
             added_epoch) SELECT '{c}', 1, d.id, d.user_id, 0 FROM device_key d;",
            a = alice.user.id,
            b = bob.user.id,
        ))
        .await
        .expect("encrypted group");
    let group = || bob.caller.request(GROUP, json!({ "s": s, "c": c }));
    let before = exec_ok(&schema, group()).await;
    assert_eq!(
        before["conversation"]["mlsGroup"]["pendingRemoves"],
        json!([])
    );

    exec_ok(&schema, request_as(&alice.caller, "alice")).await;
    let run = purge_deleted_accounts(provider.account_deletion_service().as_ref())
        .await
        .expect("run");
    assert_eq!(run.purged.len(), 1);

    let after = exec_ok(&schema, group()).await;
    assert_eq!(
        after["conversation"]["mlsGroup"]["pendingRemoves"],
        json!([{
            "principalId": alice.state.principal_id,
            "deviceId": alice.device_id.to_string(),
        }])
    );

    let input = signup_input(&schema, "alice-again", &alice.identity, 43, json!({})).await;
    let (code, message) = exec_err(
        &schema,
        anonymous_request(SIGNUP_WITH_KEY, json!({ "input": input })),
    )
    .await;
    assert_eq!(code, "VALIDATION");
    assert!(message.contains("deleted"), "{message}");
}

/// Setting the avatar keeps the file it names and deletes the account's other avatar files,
/// clearing it deletes them all, and another account's avatar URL is refused. The purge deletes
/// every file in the account's avatar directory, whatever its avatar URL names, and leaves the
/// avatar of the account that URL belongs to alone.
#[tokio::test]
async fn avatar_files_go_with_a_changed_avatar_and_with_the_account() {
    let ctx = TestCtx::new().await;
    let (schema, provider) =
        ctx.build_with_deletion_grace(Duration::zero(), RegistrationMode::InviteOnly);
    let alice_user = ctx.create_password_user("alice").await;
    let bob_user = ctx.create_user("bob").await;
    let alice = ctx.caller(&alice_user).await;
    let (a, b) = (alice_user.id, bob_user.id);
    store_avatars(a, &["avatar.gif", "avatar.jpg", "avatar.png"]);
    store_avatars(b, &["avatar.png"]);
    let set = |url: String| alice.request(SET_AVATAR, json!({ "u": url }));

    let (code, message) =
        exec_err(&schema, set(format!("/api/files/avatars/{b}/avatar.png"))).await;
    assert_eq!(code, "VALIDATION");
    assert!(message.contains("avatar_url"), "{message}");
    assert_eq!(avatar_files(a), ["avatar.gif", "avatar.jpg", "avatar.png"]);

    let own = format!("/api/files/avatars/{a}/avatar.gif");
    let data = exec_ok(&schema, set(own.clone())).await;
    assert_eq!(data["updateMyProfile"]["avatarUrl"], json!(own));
    assert_eq!(avatar_files(a), ["avatar.gif"]);

    let data = exec_ok(&schema, set(String::new())).await;
    assert_eq!(data["updateMyProfile"]["avatarUrl"], Value::Null);
    assert_eq!(avatar_files(a), Vec::<String>::new());

    // An avatar URL naming another account's file, which the mutation refuses but a row may hold.
    store_avatars(a, &["avatar.png", "avatar.webp"]);
    ctx.conn
        .execute_unprepared(&format!(
            "UPDATE \"user\" SET avatar_url = '/api/files/avatars/{b}/avatar.png' WHERE id = '{a}'"
        ))
        .await
        .expect("avatar url");
    exec_ok(&schema, request_as(&alice, "alice")).await;
    let run = purge_deleted_accounts(provider.account_deletion_service().as_ref())
        .await
        .expect("run");
    assert_eq!(run.failed, vec![]);
    assert_eq!(run.purged.len(), 1);
    assert_eq!(run.purged[0].files, 2);
    assert_eq!(avatar_files(a), Vec::<String>::new());
    assert_eq!(avatar_files(b), ["avatar.png"]);
}
