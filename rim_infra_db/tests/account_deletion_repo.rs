#![cfg(feature = "db-tests")]

mod support;

use chrono::{Duration, Utc};
use rim_core::account_deletion::repository::AccountDeletionRepository;
use rim_core::conversation::model::{ConversationId, ConversationType};
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::message::model::MessageId;
use rim_core::server::model::ServerId;
use rim_core::user::model::{User, UserId};
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::account_deletion::SeaOrmAccountDeletionRepository;
use rim_infra_db::sea_orm_impl::key_identity::SeaOrmKeyIdentityRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use sea_orm::ConnectionTrait;
use sea_orm::prelude::Uuid;

/// Run `sql` on the test database.
async fn sql(ctx: &support::TestCtx, sql: &str) {
    ctx.db.execute_unprepared(sql).await.expect(sql);
}

/// The count `sql` (a `SELECT count(*)`) answers.
async fn count(ctx: &support::TestCtx, sql: &str) -> i64 {
    let row = ctx
        .db
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Postgres,
            sql.to_string(),
        ))
        .await
        .expect(sql)
        .expect("a row");
    row.try_get_by_index::<i64>(0).expect("count")
}

fn repo(ctx: &support::TestCtx) -> SeaOrmAccountDeletionRepository {
    SeaOrmAccountDeletionRepository::new(ctx.db.clone())
}

async fn add_member(ctx: &support::TestCtx, server: ServerId, user: UserId, role: &str) {
    sql(
        ctx,
        &format!(
            "INSERT INTO server_member (id, server_id, user_id, member_role) \
             VALUES (gen_random_uuid(), '{server}', '{user}', '{role}')"
        ),
    )
    .await;
}

async fn join(ctx: &support::TestCtx, conversation: ConversationId, user: UserId) {
    sql(
        ctx,
        &format!(
            "INSERT INTO conversation_member (id, user_id, conversation_id) \
             VALUES (gen_random_uuid(), '{user}', '{conversation}')"
        ),
    )
    .await;
}

/// A plaintext message of `user` replying to `parent`.
async fn reply(
    ctx: &support::TestCtx,
    conversation: ConversationId,
    user: UserId,
    parent: MessageId,
) -> MessageId {
    let id = Uuid::from(MessageId::new());
    sql(
        ctx,
        &format!(
            "INSERT INTO message (id, conversation_id, sent_by, text, parent_message_id) \
             VALUES ('{id}', '{conversation}', '{user}', 'a reply', '{parent}')"
        ),
    )
    .await;
    id.into()
}

#[tokio::test]
async fn a_request_is_recorded_once_listed_when_due_and_cleared_by_a_cancel() {
    let ctx = support::TestCtx::new().await;
    let repo = repo(&ctx);
    let user = ctx.create_user("alice").await;
    let now = Utc::now();
    let scheduled = now + Duration::days(14);

    assert!(repo.record_request(user.id, now, scheduled).await.unwrap());
    assert!(!repo.record_request(user.id, now, now).await.unwrap());
    let standing = repo.standing(user.id).await.unwrap().unwrap();
    assert_eq!(standing.username, "alice");
    assert!(!standing.is_remote);
    assert_eq!(
        standing
            .deletion_scheduled_for
            .map(|t| t.timestamp_micros()),
        Some(scheduled.timestamp_micros())
    );
    let me = SeaOrmUserRepository::new(ctx.db.clone())
        .get_by_id(user.id)
        .await
        .unwrap()
        .unwrap();
    assert!(me.deletion_scheduled_for.is_some());

    assert_eq!(repo.due(now, 10).await.unwrap(), vec![]);
    assert_eq!(repo.due(scheduled, 10).await.unwrap(), vec![user.id]);
    assert!(!repo.claim_due(user.id, now).await.unwrap());
    assert!(repo.claim_due(user.id, scheduled).await.unwrap());

    assert!(repo.clear_request(user.id).await.unwrap());
    assert!(!repo.clear_request(user.id).await.unwrap());
    assert_eq!(repo.due(scheduled, 10).await.unwrap(), vec![]);
}

#[tokio::test]
async fn only_a_workspace_with_other_members_and_no_other_owner_blocks_its_owner() {
    let ctx = support::TestCtx::new().await;
    let repo = repo(&ctx);
    let (alice, bob) = (ctx.create_user("alice").await, ctx.create_user("bob").await);
    let shared = ctx.create_server("Shared").await;
    add_member(&ctx, shared.id, alice.id, "owner").await;
    add_member(&ctx, shared.id, bob.id, "member").await;
    let alone = ctx.create_server("Alone").await;
    add_member(&ctx, alone.id, alice.id, "owner").await;
    let co_owned = ctx.create_server("Co-owned").await;
    add_member(&ctx, co_owned.id, alice.id, "owner").await;
    add_member(&ctx, co_owned.id, bob.id, "owner").await;
    let joined = ctx.create_server("Joined").await;
    add_member(&ctx, joined.id, bob.id, "owner").await;
    add_member(&ctx, joined.id, alice.id, "member").await;

    let blocking = repo.owned_shared_workspaces(alice.id).await.unwrap();
    assert_eq!(blocking.len(), 1);
    assert_eq!(blocking[0].id, shared.id);
    assert_eq!(blocking[0].name, "Shared");
}

#[tokio::test]
async fn the_workspace_step_deletes_sole_workspaces_leaves_the_others_and_revokes_invites() {
    let ctx = support::TestCtx::new().await;
    let repo = repo(&ctx);
    let (alice, bob) = (ctx.create_user("alice").await, ctx.create_user("bob").await);
    let alone = ctx.create_server("Alone").await;
    add_member(&ctx, alone.id, alice.id, "owner").await;
    let channel = ctx
        .create_conversation(alone.id, ConversationType::Channel)
        .await;
    ctx.create_message(channel.id, alice.id, "only mine").await;
    sql(
        &ctx,
        &format!(
            "INSERT INTO custom_emoji (id, server_id, name, image_url, uploaded_by) \
             VALUES (gen_random_uuid(), '{}', 'wave', 'emoji/{}/wave.png', '{}')",
            alone.id, alone.id, alice.id
        ),
    )
    .await;
    let shared = ctx.create_server("Shared").await;
    add_member(&ctx, shared.id, bob.id, "owner").await;
    add_member(&ctx, shared.id, alice.id, "member").await;
    let encrypted = ctx
        .create_conversation(shared.id, ConversationType::Group)
        .await;
    sql(
        &ctx,
        &format!(
            "UPDATE conversation SET encrypted_at = now() WHERE id = '{}'",
            encrypted.id
        ),
    )
    .await;
    join(&ctx, encrypted.id, alice.id).await;
    join(&ctx, encrypted.id, bob.id).await;
    sql(
        &ctx,
        &format!(
            "INSERT INTO invite (id, server_id, created_by_user_id, token_hash) \
             VALUES (gen_random_uuid(), '{}', '{}', 'hash-a')",
            shared.id, alice.id
        ),
    )
    .await;

    sql(
        &ctx,
        &format!(
            "INSERT INTO workspace_join_request (id, server_id, user_id, expires_at) \
             VALUES (gen_random_uuid(), '{}', '{}', now() + interval '30 days')",
            shared.id, alice.id
        ),
    )
    .await;

    let step = repo.purge_workspaces(alice.id, Utc::now()).await.unwrap();

    assert!(step.counts.contains(&("workspaces_deleted", 1)));
    assert!(step.counts.contains(&("join_requests", 1)));
    assert!(step.counts.contains(&("workspace_memberships", 1)));
    assert!(step.counts.contains(&("conversation_memberships", 1)));
    assert!(step.counts.contains(&("invites_revoked", 1)));
    assert_eq!(
        step.storage_keys,
        vec![format!("emoji/{}/wave.png", alone.id)]
    );
    assert_eq!(step.left_workspaces, vec![shared.id]);
    assert_eq!(step.left_conversations.len(), 1);
    assert_eq!(step.left_conversations[0].conversation_id, encrypted.id);
    assert_eq!(step.left_conversations[0].member_user_ids, vec![bob.id]);
    assert_eq!(
        count(
            &ctx,
            &format!("SELECT count(*) FROM server WHERE id = '{}'", alone.id)
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &ctx,
            &format!(
                "SELECT count(*) FROM server_member WHERE user_id = '{}'",
                alice.id
            )
        )
        .await,
        0
    );
    assert_eq!(
        count(&ctx, "SELECT count(*) FROM invite WHERE revoked_at IS NULL").await,
        0
    );
}

#[tokio::test]
async fn the_message_step_deletes_plaintext_keeps_replied_parents_as_deleted_and_keeps_ciphertext()
{
    let ctx = support::TestCtx::new().await;
    let repo = repo(&ctx);
    let (alice, bob) = (ctx.create_user("alice").await, ctx.create_user("bob").await);
    let server = ctx.create_server("Shared").await;
    let channel = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let alone = ctx
        .create_message(channel.id, alice.id, "secret plan")
        .await;
    let parent = ctx.create_message(channel.id, alice.id, "question").await;
    let bobs_reply = reply(&ctx, channel.id, bob.id, parent.id).await;
    // Alice's reply to her own message that Bob answered: both stay as deleted.
    let grandparent = ctx.create_message(channel.id, alice.id, "start").await;
    let middle = reply(&ctx, channel.id, alice.id, grandparent.id).await;
    reply(&ctx, channel.id, bob.id, middle).await;
    // A thread Alice started that only she replied in goes with its replies.
    let lonely_root = ctx.create_message(channel.id, alice.id, "lonely").await;
    let thread = ctx.create_thread(channel.id).await;
    sql(
        &ctx,
        &format!(
            "UPDATE message_thread SET first_message_id = '{}' WHERE id = '{}'",
            lonely_root.id, thread.id
        ),
    )
    .await;
    let thread_reply = ctx
        .create_thread_message(channel.id, thread.id, alice.id, "me again")
        .await;
    let encrypted = Uuid::from(MessageId::new());
    sql(
        &ctx,
        &format!(
            "INSERT INTO message (id, conversation_id, sent_by, encrypted, mls_ciphertext, \
             mls_incarnation, mls_epoch) \
             VALUES ('{encrypted}', '{}', '{}', true, '\\x0102', 0, 1)",
            channel.id, alice.id
        ),
    )
    .await;
    let bobs = ctx.create_message(channel.id, bob.id, "bob's own").await;
    for (message, user) in [(bobs.id, alice.id), (parent.id, bob.id)] {
        sql(
            &ctx,
            &format!(
                "INSERT INTO message_reaction (id, message_id, user_id, emoji) \
                 VALUES (gen_random_uuid(), '{message}', '{user}', '+1')"
            ),
        )
        .await;
    }
    sql(
        &ctx,
        &format!(
            "INSERT INTO message_attachment (id, storage_url, message_id, file_size, file_name, \
             mime_type) VALUES (gen_random_uuid(), 'uploads/{}/a.png', '{}', 1, 'a.png', \
             'image/png')",
            alice.id, alone.id
        ),
    )
    .await;

    let step = repo.purge_messages(alice.id).await.unwrap();

    assert_eq!(
        step.storage_keys,
        vec![format!("uploads/{}/a.png", alice.id)]
    );
    assert!(step.counts.contains(&("messages_kept_as_deleted", 3)));
    // The thread's own reply goes with the thread.
    assert!(step.counts.contains(&("messages_deleted", 2)));
    assert!(step.counts.contains(&("threads", 1)));
    assert!(step.counts.contains(&("encrypted_messages_kept", 1)));
    let gone = |id: Uuid| format!("SELECT count(*) FROM message WHERE id = '{id}'");
    for id in [
        alone.id.into(),
        lonely_root.id.into(),
        thread_reply.id.into(),
    ] {
        assert_eq!(count(&ctx, &gone(id)).await, 0);
    }
    for id in [parent.id, grandparent.id, middle] {
        assert_eq!(
            count(
                &ctx,
                &format!(
                    "SELECT count(*) FROM message WHERE id = '{id}' AND is_deleted \
                     AND text IS NULL AND sent_by = '{}'",
                    alice.id
                )
            )
            .await,
            1,
            "{id}"
        );
    }
    assert_eq!(count(&ctx, &gone(bobs_reply.into())).await, 1);
    assert_eq!(
        count(
            &ctx,
            &format!(
                "SELECT count(*) FROM message WHERE id = '{encrypted}' \
                 AND mls_ciphertext IS NOT NULL AND NOT is_deleted"
            )
        )
        .await,
        1
    );
    assert_eq!(
        count(&ctx, "SELECT count(*) FROM message_reaction").await,
        0
    );
    assert_eq!(
        count(
            &ctx,
            "SELECT count(*) FROM message WHERE search_tsv @@ to_tsquery('english', 'secret')"
        )
        .await,
        0
    );
}

#[tokio::test]
async fn the_credential_step_ends_sessions_and_revokes_devices_and_the_personal_step_keeps_used_macros()
 {
    let ctx = support::TestCtx::new().await;
    let repo = repo(&ctx);
    let alice = ctx.create_key_only_user("alice", "rimp1_alice").await;
    let server = ctx.create_server("Shared").await;
    sql(
        &ctx,
        &format!(
            "INSERT INTO session (id, user_id, token_hash, expires_at) \
             VALUES (gen_random_uuid(), '{0}', 'tok', now() + interval '1 day'); \
             INSERT INTO device_key (id, user_id, device_id, public_key, name, cert_payload, \
             cert_signature, key_seq, key_hash) VALUES (gen_random_uuid(), '{0}', \
             gen_random_uuid(), 'ed25519:k', 'laptop', 'p', 's', 0, decode(repeat('00', 32), 'hex')); \
             INSERT INTO key_log_entry (user_id, seq, entry_type, payload, signature) \
             VALUES ('{0}', 0, 'inception', 'p', 's'); \
             INSERT INTO account_password (user_id, registration, share_sealed) \
             VALUES ('{0}', '\\x01', '\\x02'); \
             INSERT INTO password_rewrap (id, user_id, rewrap) \
             VALUES (gen_random_uuid(), '{0}', '\\x03'); \
             INSERT INTO password_login (id, user_id, purpose, state, expires_at) \
             VALUES (gen_random_uuid(), '{0}', 'login', '\\x04', now()); \
             INSERT INTO password_reset (id, user_id, expires_at) \
             VALUES (gen_random_uuid(), '{0}', now()); \
             INSERT INTO password_ticket (token_hash, user_id, expires_at) \
             VALUES ('t', '{0}', now()); \
             INSERT INTO macro (id, server_id, user_id, name) VALUES \
             ('0199d1a0-0000-7000-8000-00000000000a', '{1}', '{0}', 'used'), \
             (gen_random_uuid(), '{1}', '{0}', 'unused'); \
             INSERT INTO pipeline (id, server_id, name, trigger_data, actions_data, created_by) \
             VALUES (gen_random_uuid(), '{1}', 'p', '{{}}', \
             '[{{\"ExecuteCommand\":{{\"command_id\":\"macro.0199d1a0-0000-7000-8000-00000000000a\"}}}}]', \
             '{0}'); \
             INSERT INTO keybind (id, scope, subject_id, keystroke, command_id, context) \
             VALUES (gen_random_uuid(), 'user', '{0}', 'Ctrl+K', 'x', 'global');",
            alice.id, server.id
        ),
    )
    .await;

    let credentials = repo.purge_credentials(alice.id, Utc::now()).await.unwrap();
    assert!(credentials.counts.contains(&("sessions", 1)));
    assert!(credentials.counts.contains(&("devices_revoked", 1)));
    assert!(credentials.counts.contains(&("key_log_entries", 1)));
    for kind in [
        "account_password",
        "password_rewraps",
        "password_logins",
        "password_resets",
        "password_tickets",
    ] {
        assert!(credentials.counts.contains(&(kind, 1)), "{kind}");
    }
    assert_eq!(
        count(
            &ctx,
            "SELECT count(*) FROM device_key WHERE revoked_at IS NOT NULL"
        )
        .await,
        1
    );

    let personal = repo.purge_personal_data(alice.id).await.unwrap();
    assert!(personal.counts.contains(&("macros", 1)));
    assert!(personal.counts.contains(&("macros_kept", 1)));
    assert!(personal.counts.contains(&("pipelines_kept", 1)));
    assert!(personal.counts.contains(&("keybinds", 1)));
}

/// A user whose id starts with the same 8 hex digits as `other`'s.
async fn create_user_sharing_the_id_prefix(
    ctx: &support::TestCtx,
    other: &User,
    username: &str,
) -> UserId {
    let prefix = &Uuid::from(other.id).simple().to_string()[..8];
    let id = format!("{prefix}-ffff-7fff-8fff-ffffffffffff");
    sql(
        ctx,
        &format!(
            "INSERT INTO \"user\" (id, username, principal_id, identity_key, created_at, \
             updated_at) VALUES ('{id}', '{username}', 'rimp1_{username}', 'ed25519:{username}', \
             now(), now())"
        ),
    )
    .await;
    Uuid::parse_str(&id).expect("uuid").into()
}

#[tokio::test]
async fn a_tombstone_frees_the_username_and_is_found_by_id_only() {
    let ctx = support::TestCtx::new().await;
    let repo = repo(&ctx);
    let users = SeaOrmUserRepository::new(ctx.db.clone());
    let keys = SeaOrmKeyIdentityRepository::new(ctx.db.clone());
    let alice = ctx.create_key_only_user("alice", "rimp1_alice").await;
    sql(
        &ctx,
        &format!(
            "UPDATE \"user\" SET email = 'alice@example.com', bio = 'hi', \
             avatar_url = '/api/files/avatars/{0}/avatar.png', terms_accepted_url = 'https://t', \
             minimum_age_confirmed = 16 WHERE id = '{0}'",
            alice.id
        ),
    )
    .await;
    let twin = create_user_sharing_the_id_prefix(&ctx, &alice, "twin").await;

    // The avatar files are found in the account's avatar directory, not through its URL.
    let step = repo.tombstone(alice.id, Utc::now()).await.unwrap();
    assert!(step.storage_keys.is_empty());
    repo.tombstone(twin, Utc::now()).await.unwrap();

    let tombstone = users.get_by_id(alice.id).await.unwrap().unwrap();
    let prefix = &Uuid::from(alice.id).simple().to_string()[..8];
    assert_eq!(tombstone.username, format!("deleted-{prefix}"));
    assert_eq!(tombstone.display_name.as_deref(), Some("Deleted account"));
    assert_eq!(tombstone.email, None);
    assert_eq!(tombstone.avatar_url, None);
    assert_eq!(tombstone.bio, None);
    let twin_row = users.get_by_id(twin).await.unwrap().unwrap();
    assert!(twin_row.username.starts_with(&format!("deleted-{prefix}")));
    assert_ne!(twin_row.username, tombstone.username);

    assert!(users.find_by_username("alice").await.unwrap().is_none());
    assert!(
        users
            .find_by_username(&tombstone.username)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        users
            .find_by_email("alice@example.com")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        keys.find_user_by_principal_id("rimp1_alice")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        count(
            &ctx,
            &format!(
                "SELECT count(*) FROM \"user\" WHERE id = '{}' AND identity_key IS NULL \
                 AND terms_accepted_url IS NULL AND minimum_age_confirmed IS NULL \
                 AND deleted_at IS NOT NULL",
                alice.id
            )
        )
        .await,
        1
    );
    // The username is free for a new account.
    ctx.create_user("alice").await;
    let standing = repo.standing(alice.id).await.unwrap().unwrap();
    assert!(standing.deleted_at.is_some());
}
