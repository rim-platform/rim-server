#![cfg(feature = "db-tests")]

mod support;

use chrono::{Duration, DurationRound, Utc};
use migration::{Migrator, MigratorTrait};
use rim_core::account_deletion::model::TOMBSTONE_DISPLAY_NAME;
use rim_core::account_deletion::repository::AccountDeletionRepository;
use rim_core::bot::model::{
    BotFeedPosition, BotPatch, NewBot, NewBotToken, NewIncomingWebhook, WebhookPreset,
};
use rim_core::bot::repository::BotRepository;
use rim_core::conversation::model::ConversationId;
use rim_core::repository::error::RepoErrorKind;
use rim_core::server::model::{Server, ServerId};
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::{User, UserHandle, UserId};
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::account_deletion::SeaOrmAccountDeletionRepository;
use rim_infra_db::sea_orm_impl::bot::SeaOrmBotRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use sea_orm::prelude::Uuid;
use sea_orm::{ConnectionTrait, DbBackend, Statement};

const MIGRATION_NAME: &str = "m20261106_070423_create_bot";

/// The count a `SELECT count(*) AS n` of `sql` answers.
async fn count(ctx: &support::TestCtx, sql: &str, id: Uuid) -> i64 {
    ctx.db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [id.into()],
        ))
        .await
        .expect(sql)
        .expect("a row")
        .try_get::<i64>("", "n")
        .expect("n")
}

fn repo(ctx: &support::TestCtx) -> SeaOrmBotRepository {
    SeaOrmBotRepository::new(ctx.db.clone())
}

fn new_bot(server_id: ServerId, username: &str, created_by: UserId) -> NewBot {
    NewBot {
        server_id,
        username: username.to_string(),
        name: "Helper".to_string(),
        run_by: "Ada".to_string(),
        reads_go_to: "Nowhere".to_string(),
        created_by,
        sees_member_joins: false,
    }
}

fn new_token(bot_user_id: UserId, hash: &str, created_by: UserId) -> NewBotToken {
    NewBotToken {
        bot_user_id,
        label: "ci".to_string(),
        token_hash: hash.to_string(),
        created_by,
    }
}

/// A workspace and a person who adds bots to it.
async fn workspace(ctx: &support::TestCtx, name: &str) -> (Server, User) {
    (
        ctx.create_server(name).await,
        ctx.create_user(&format!("{name}-ada")).await,
    )
}

#[tokio::test]
async fn a_bot_is_a_user_of_its_own_kind_with_no_credentials() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "kind").await;
    let bots = repo(&ctx);

    let bot = bots
        .create_bot(new_bot(server.id, "bot-helper-1", ada.id))
        .await
        .unwrap();

    assert_eq!(bot.name, "Helper");
    assert_eq!(bot.server_id, server.id);
    assert_eq!(bot.run_by, "Ada");
    assert_eq!(bot.reads_go_to, "Nowhere");
    assert_eq!(bot.created_by, ada.id);
    assert_eq!(bot.added_by_name.as_deref(), Some("kind-ada"));
    assert_eq!(bot.deactivated_at, None);
    assert_eq!(
        count(
            &ctx,
            r#"SELECT count(*) AS n FROM "user" WHERE id = $1 AND kind = 'bot'
                 AND email IS NULL AND principal_id IS NULL AND identity_key IS NULL
                 AND terms_accepted_at IS NULL AND terms_accepted_url IS NULL
                 AND deleted_at IS NULL"#,
            bot.user_id.into(),
        )
        .await,
        1
    );

    let user = SeaOrmUserRepository::new(ctx.db.clone())
        .get_by_id(bot.user_id)
        .await
        .unwrap()
        .unwrap();
    assert!(user.is_bot);
    assert_eq!(user.display_name.as_deref(), Some("Helper"));
    assert_eq!(user.handle, UserHandle::Unknown);
    assert!(!ada.is_bot);
    assert_eq!(ada.handle, UserHandle::Local);
}

#[tokio::test]
async fn a_person_still_needs_a_principal_and_no_other_kind_is_accepted() {
    let ctx = support::TestCtx::new().await;
    let refused = ctx
        .db
        .execute_unprepared(
            r#"INSERT INTO "user" (id, username, kind) VALUES (gen_random_uuid(), 'nokey', 'local')"#,
        )
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("chk_user_principal"),
        "{refused}"
    );
    let refused = ctx
        .db
        .execute_unprepared(
            r#"INSERT INTO "user" (id, username, kind) VALUES (gen_random_uuid(), 'other', 'robot')"#,
        )
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("chk_user_kind"), "{refused}");
}

#[tokio::test]
async fn a_taken_username_is_a_conflict() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "taken").await;
    let bots = repo(&ctx);
    bots.create_bot(new_bot(server.id, "bot-same", ada.id))
        .await
        .unwrap();
    let err = bots
        .create_bot(new_bot(server.id, "BOT-SAME", ada.id))
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::Conflict));
}

#[tokio::test]
async fn lists_counts_and_updates() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "lists").await;
    let other = ctx.create_server("lists-other").await;
    let bots = repo(&ctx);
    let first = bots
        .create_bot(new_bot(server.id, "bot-first", ada.id))
        .await
        .unwrap();
    let second = bots
        .create_bot(new_bot(server.id, "bot-second", ada.id))
        .await
        .unwrap();
    let elsewhere = bots
        .create_bot(new_bot(other.id, "bot-elsewhere", ada.id))
        .await
        .unwrap();

    let listed = bots.list_by_server(server.id).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|b| b.server_id == server.id));
    assert_eq!(bots.count_active(server.id).await.unwrap(), 2);

    let among = bots
        .bots_among(&[first.user_id, ada.id, elsewhere.user_id])
        .await
        .unwrap();
    assert_eq!(among.len(), 2);
    assert!(!among.contains(&ada.id));
    assert!(bots.bots_among(&[]).await.unwrap().is_empty());
    let by_users = bots.list_by_users(&[second.user_id, ada.id]).await.unwrap();
    assert_eq!(by_users.len(), 1);
    assert_eq!(by_users[0].user_id, second.user_id);

    let renamed = bots
        .update_bot(
            first.user_id,
            BotPatch {
                name: Some("Renamed".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(renamed.name, "Renamed");
    assert_eq!(renamed.run_by, "Ada");
    let changed = bots
        .update_bot(
            first.user_id,
            BotPatch {
                run_by: Some("Grace".to_string()),
                reads_go_to: Some("A model".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(changed.name, "Renamed");
    assert_eq!(changed.run_by, "Grace");
    assert_eq!(changed.reads_go_to, "A model");
    assert!(matches!(
        bots.update_bot(ada.id, BotPatch::default())
            .await
            .unwrap_err()
            .kind(),
        RepoErrorKind::NotFound
    ));

    assert!(
        bots.set_deactivated(first.user_id, Utc::now())
            .await
            .unwrap()
    );
    assert!(
        !bots
            .set_deactivated(first.user_id, Utc::now())
            .await
            .unwrap()
    );
    assert_eq!(bots.count_active(server.id).await.unwrap(), 1);
    assert_eq!(bots.list_by_server(server.id).await.unwrap().len(), 2);

    assert!(bots.lock_bot(first.user_id).await.unwrap());
    assert!(!bots.lock_bot(ada.id).await.unwrap());
    bots.lock_server_bots(server.id).await.unwrap();
}

#[tokio::test]
async fn tokens_round_trip_with_a_unique_hash() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "tokens").await;
    let bots = repo(&ctx);
    let bot = bots
        .create_bot(new_bot(server.id, "bot-tokens", ada.id))
        .await
        .unwrap();

    let token = bots
        .create_token(new_token(bot.user_id, "hash-1", ada.id))
        .await
        .unwrap();
    assert_eq!(token.label, "ci");
    assert_eq!(token.last_used_at, None);
    assert_eq!(token.revoked_at, None);
    let second = bots
        .create_token(new_token(bot.user_id, "hash-2", ada.id))
        .await
        .unwrap();
    assert!(matches!(
        bots.create_token(new_token(bot.user_id, "hash-1", ada.id))
            .await
            .unwrap_err()
            .kind(),
        RepoErrorKind::Conflict
    ));

    assert_eq!(bots.find_token(token.id).await.unwrap().unwrap(), token);
    let listed = bots.list_tokens(bot.user_id).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, second.id, "newest first");
    assert_eq!(bots.count_active_tokens(bot.user_id).await.unwrap(), 2);

    let holder = bots.find_token_holder("hash-1").await.unwrap().unwrap();
    assert_eq!(holder.token.id, token.id);
    assert_eq!(holder.server_id, server.id);
    assert_eq!(holder.bot_deactivated_at, None);
    assert!(bots.find_token_holder("nope").await.unwrap().is_none());

    let at = Utc::now().duration_trunc(Duration::seconds(1)).unwrap();
    bots.touch_token(token.id, at).await.unwrap();
    assert_eq!(
        bots.find_token(token.id)
            .await
            .unwrap()
            .unwrap()
            .last_used_at,
        Some(at)
    );

    let revoked = bots.revoke_token(token.id, at).await.unwrap().unwrap();
    assert_eq!(revoked.revoked_at, Some(at));
    let again = bots
        .revoke_token(token.id, at + Duration::hours(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.revoked_at, Some(at), "the first revocation stands");
    assert_eq!(bots.count_active_tokens(bot.user_id).await.unwrap(), 1);

    assert_eq!(bots.revoke_all_tokens(bot.user_id, at).await.unwrap(), 1);
    assert_eq!(bots.revoke_all_tokens(bot.user_id, at).await.unwrap(), 0);
    assert_eq!(bots.count_active_tokens(bot.user_id).await.unwrap(), 0);

    bots.set_deactivated(bot.user_id, at).await.unwrap();
    assert_eq!(
        bots.find_token_holder("hash-2")
            .await
            .unwrap()
            .unwrap()
            .bot_deactivated_at,
        Some(at)
    );
}

#[tokio::test]
async fn deleting_a_workspace_takes_its_bots_and_leaves_a_tombstone() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "gone").await;
    let kept = ctx.create_server("kept").await;
    let bots = repo(&ctx);
    let bot = bots
        .create_bot(new_bot(server.id, "bot-gone", ada.id))
        .await
        .unwrap();
    let other = bots
        .create_bot(new_bot(kept.id, "bot-kept", ada.id))
        .await
        .unwrap();
    bots.create_token(new_token(bot.user_id, "gone-hash", ada.id))
        .await
        .unwrap();

    SeaOrmServerRepository::new(ctx.db.clone())
        .delete_workspaces(&[server.id])
        .await
        .unwrap();

    assert!(bots.find_bot(bot.user_id).await.unwrap().is_none());
    assert!(bots.find_token_holder("gone-hash").await.unwrap().is_none());
    assert_eq!(
        count(
            &ctx,
            "SELECT count(*) AS n FROM bot_token WHERE bot_user_id = $1",
            bot.user_id.into(),
        )
        .await,
        0
    );
    let users = SeaOrmUserRepository::new(ctx.db.clone());
    let tombstone = users.get_by_id(bot.user_id).await.unwrap().unwrap();
    assert_eq!(
        tombstone.display_name.as_deref(),
        Some(TOMBSTONE_DISPLAY_NAME)
    );
    assert!(tombstone.username.starts_with("deleted-"));
    assert!(users.find_by_username("bot-gone").await.unwrap().is_none());

    assert!(bots.find_bot(other.user_id).await.unwrap().is_some());
    assert_eq!(
        count(
            &ctx,
            r#"SELECT count(*) AS n FROM "user" WHERE id = $1 AND deleted_at IS NULL"#,
            other.user_id.into(),
        )
        .await,
        1
    );
}

#[tokio::test]
async fn removing_a_bots_user_row_cascades_to_its_tokens() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "cascade").await;
    let bots = repo(&ctx);
    let bot = bots
        .create_bot(new_bot(server.id, "bot-cascade", ada.id))
        .await
        .unwrap();
    bots.create_token(new_token(bot.user_id, "cascade-hash", ada.id))
        .await
        .unwrap();

    ctx.db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"DELETE FROM "user" WHERE id = $1"#,
            [Uuid::from(bot.user_id).into()],
        ))
        .await
        .unwrap();

    assert!(bots.find_bot(bot.user_id).await.unwrap().is_none());
    assert!(
        bots.find_token_holder("cascade-hash")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_deleted_account_stays_named_on_the_bots_it_added() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "named").await;
    let bots = repo(&ctx);
    let bot = bots
        .create_bot(new_bot(server.id, "bot-named", ada.id))
        .await
        .unwrap();
    bots.create_token(new_token(bot.user_id, "named-hash", ada.id))
        .await
        .unwrap();

    SeaOrmAccountDeletionRepository::new(ctx.db.clone())
        .tombstone(ada.id, Utc::now())
        .await
        .unwrap();

    let found = bots.find_bot(bot.user_id).await.unwrap().unwrap();
    assert_eq!(found.created_by, ada.id);
    assert_eq!(found.added_by_name.as_deref(), Some(TOMBSTONE_DISPLAY_NAME));
}

#[tokio::test]
async fn a_bot_does_not_make_a_workspace_shared_when_its_owner_leaves() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "sole").await;
    let (shared, _) = workspace(&ctx, "shared").await;
    let carol = ctx.create_user("carol").await;
    let members = SeaOrmServerMemberRepository::new(ctx.db.clone());
    let bots = repo(&ctx);

    members
        .add_member(server.id, ada.id, ServerMemberRole::Owner)
        .await
        .unwrap();
    let bot = bots
        .create_bot(new_bot(server.id, "bot-sole", ada.id))
        .await
        .unwrap();
    members
        .add_member(server.id, bot.user_id, ServerMemberRole::Guest)
        .await
        .unwrap();
    members
        .add_member(shared.id, ada.id, ServerMemberRole::Owner)
        .await
        .unwrap();
    members
        .add_member(shared.id, carol.id, ServerMemberRole::Member)
        .await
        .unwrap();

    let deletion = SeaOrmAccountDeletionRepository::new(ctx.db.clone());
    let owned = deletion.owned_shared_workspaces(ada.id).await.unwrap();
    assert_eq!(owned.len(), 1);
    assert_eq!(owned[0].id, shared.id);

    deletion.purge_workspaces(ada.id, Utc::now()).await.unwrap();
    let servers = SeaOrmServerRepository::new(ctx.db.clone());
    assert!(servers.get_by_id(server.id).await.unwrap().is_none());
    assert!(servers.get_by_id(shared.id).await.unwrap().is_some());
}

#[tokio::test]
async fn reverting_the_migration_waits_for_bots_to_go_and_brings_the_constraints_back() {
    let ctx = support::TestCtx::new().await;
    let names: Vec<String> = Migrator::migrations()
        .iter()
        .map(|m| m.name().to_string())
        .collect();
    let index = names
        .iter()
        .position(|n| n == MIGRATION_NAME)
        .expect("the migration is registered");
    let steps = u32::try_from(names.len() - index).expect("step count fits u32");
    let (server, ada) = workspace(&ctx, "revert").await;
    let bot = repo(&ctx)
        .create_bot(new_bot(server.id, "bot-revert", ada.id))
        .await
        .unwrap();

    let conn = support::db_conn().await;
    let refused = support::roll_back(&conn, steps).await.unwrap_err();
    assert!(
        refused.to_string().contains("bot accounts exist"),
        "{refused}"
    );

    conn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"DELETE FROM "user" WHERE id = $1"#,
        [Uuid::from(bot.user_id).into()],
    ))
    .await
    .unwrap();
    support::roll_back(&conn, steps).await.unwrap();
    let refused = conn
        .execute_unprepared(
            r#"INSERT INTO "user" (id, username, kind, principal_id, identity_key)
               VALUES (gen_random_uuid(), 'old-kind', 'bot', 'rimp1_x', 'ed25519:x')"#,
        )
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("chk_user_kind"), "{refused}");

    support::migrate_up(&conn).await.unwrap();
    repo(&ctx)
        .create_bot(new_bot(server.id, "bot-again", ada.id))
        .await
        .unwrap();
}

/// A channel of `server`, written directly.
async fn channel_of(ctx: &support::TestCtx, server: &Server) -> ConversationId {
    let id = ConversationId::new();
    ctx.db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO conversation (id, conversation_type, server_id) \
             VALUES ($1, 'channel', $2)",
            [Uuid::from(id).into(), Uuid::from(server.id).into()],
        ))
        .await
        .unwrap();
    id
}

fn new_webhook(
    server: &Server,
    channel: ConversationId,
    bot: UserId,
    hash: &str,
    created_by: UserId,
) -> NewIncomingWebhook {
    NewIncomingWebhook {
        server_id: server.id,
        conversation_id: channel,
        bot_user_id: bot,
        preset: WebhookPreset::Github,
        template: None,
        token_hash: hash.to_string(),
        created_by,
    }
}

#[tokio::test]
async fn a_channel_has_one_webhook_that_is_not_revoked() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "hook-one").await;
    let bots = repo(&ctx);
    let bot = bots
        .create_bot(new_bot(server.id, "bot-hook-one", ada.id))
        .await
        .unwrap();
    let channel = channel_of(&ctx, &server).await;

    let first = bots
        .create_webhook(new_webhook(&server, channel, bot.user_id, "h1", ada.id))
        .await
        .unwrap();
    assert_eq!(first.preset, WebhookPreset::Github);
    assert_eq!(first.last_used_at, None);
    assert_eq!(first.revoked_at, None);
    assert_eq!(
        bots.find_active_webhook(channel).await.unwrap().unwrap().id,
        first.id
    );

    let refused = bots
        .create_webhook(new_webhook(&server, channel, bot.user_id, "h2", ada.id))
        .await
        .unwrap_err();
    assert_eq!(refused.kind(), RepoErrorKind::Conflict);

    // A hash is unique across channels too.
    let other = channel_of(&ctx, &server).await;
    let refused = bots
        .create_webhook(new_webhook(&server, other, bot.user_id, "h1", ada.id))
        .await
        .unwrap_err();
    assert_eq!(refused.kind(), RepoErrorKind::Conflict);

    let at = Utc::now().duration_trunc(Duration::seconds(1)).unwrap();
    let revoked = bots.revoke_webhook(first.id, at).await.unwrap().unwrap();
    assert_eq!(revoked.revoked_at, Some(at));
    assert_eq!(bots.find_active_webhook(channel).await.unwrap(), None);
    let later = at + Duration::hours(1);
    let again = bots.revoke_webhook(first.id, later).await.unwrap().unwrap();
    assert_eq!(again.revoked_at, Some(at), "the first revocation stands");

    let second = bots
        .create_webhook(new_webhook(&server, channel, bot.user_id, "h2", ada.id))
        .await
        .unwrap();
    let listed = bots.list_webhooks(server.id).await.unwrap();
    assert_eq!(
        listed.iter().map(|w| w.id).collect::<Vec<_>>(),
        [second.id, first.id],
        "newest first, revoked ones included"
    );
    assert!(
        bots.revoke_webhook(rim_core::bot::model::IncomingWebhookId::new(), at)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_webhooks_format_is_stored_changed_and_kept_off_a_revoked_one() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "hook-format").await;
    let bots = repo(&ctx);
    let bot = bots
        .create_bot(new_bot(server.id, "bot-hook-format", ada.id))
        .await
        .unwrap();
    let channel = channel_of(&ctx, &server).await;
    let format = serde_json::json!({ "version": 1, "rules": [{ "post": "Deployed {version}" }] });
    let created = bots
        .create_webhook(NewIncomingWebhook {
            preset: WebhookPreset::Plain,
            template: Some(format.clone()),
            ..new_webhook(&server, channel, bot.user_id, "fmt", ada.id)
        })
        .await
        .unwrap();
    assert_eq!(created.template, Some(format.clone()));
    let held = bots.find_webhook_holder("fmt").await.unwrap().unwrap();
    assert_eq!(held.webhook.template, Some(format));

    let github = bots
        .update_webhook_format(created.id, WebhookPreset::Github, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (github.preset, github.template),
        (WebhookPreset::Github, None)
    );
    let other = serde_json::json!({ "version": 1, "rules": [{ "post": "x" }] });
    let changed = bots
        .update_webhook_format(created.id, WebhookPreset::Plain, Some(other.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.template, Some(other.clone()));
    assert_eq!(
        bots.find_webhook_holder("fmt")
            .await
            .unwrap()
            .unwrap()
            .webhook
            .id,
        created.id,
        "the same token"
    );

    bots.revoke_webhook(created.id, Utc::now()).await.unwrap();
    assert_eq!(
        bots.update_webhook_format(created.id, WebhookPreset::Plain, None)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        bots.find_webhook(created.id)
            .await
            .unwrap()
            .unwrap()
            .template,
        Some(other),
        "a revoked webhook keeps what it had"
    );
    assert_eq!(
        bots.update_webhook_format(
            rim_core::bot::model::IncomingWebhookId::new(),
            WebhookPreset::Plain,
            None
        )
        .await
        .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_webhook_is_found_by_its_hash_with_its_bots_standing() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "hook-find").await;
    let bots = repo(&ctx);
    let bot = bots
        .create_bot(new_bot(server.id, "bot-hook-find", ada.id))
        .await
        .unwrap();
    let channel = channel_of(&ctx, &server).await;
    let webhook = bots
        .create_webhook(new_webhook(&server, channel, bot.user_id, "find", ada.id))
        .await
        .unwrap();

    assert_eq!(bots.find_webhook_holder("nothing").await.unwrap(), None);
    let holder = bots.find_webhook_holder("find").await.unwrap().unwrap();
    assert_eq!(holder.webhook.id, webhook.id);
    assert_eq!(holder.bot_deactivated_at, None);

    let at = Utc::now().duration_trunc(Duration::seconds(1)).unwrap();
    bots.touch_webhook(webhook.id, at).await.unwrap();
    bots.set_deactivated(bot.user_id, at).await.unwrap();
    let holder = bots.find_webhook_holder("find").await.unwrap().unwrap();
    assert_eq!(holder.webhook.last_used_at, Some(at));
    assert_eq!(holder.bot_deactivated_at, Some(at));
}

#[tokio::test]
async fn revoking_all_of_a_bots_webhooks_leaves_those_of_others_and_earlier_stamps() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "hook-all").await;
    let bots = repo(&ctx);
    let bot = bots
        .create_bot(new_bot(server.id, "bot-hook-all", ada.id))
        .await
        .unwrap();
    let other = bots
        .create_bot(new_bot(server.id, "bot-hook-other", ada.id))
        .await
        .unwrap();
    let webhook_on = async |who: UserId, hash: &str| {
        let channel = channel_of(&ctx, &server).await;
        bots.create_webhook(new_webhook(&server, channel, who, hash, ada.id))
            .await
            .unwrap()
    };
    let first = webhook_on(bot.user_id, "all-1").await;
    let second = webhook_on(bot.user_id, "all-2").await;
    let earlier = webhook_on(bot.user_id, "all-3").await;
    let others = webhook_on(other.user_id, "all-4").await;

    let at = Utc::now().duration_trunc(Duration::seconds(1)).unwrap();
    let before = at - Duration::hours(1);
    bots.revoke_webhook(earlier.id, before).await.unwrap();

    assert_eq!(bots.revoke_all_webhooks(bot.user_id, at).await.unwrap(), 2);
    assert_eq!(bots.revoke_all_webhooks(bot.user_id, at).await.unwrap(), 0);
    let revoked_at = async |id| bots.find_webhook(id).await.unwrap().unwrap().revoked_at;
    assert_eq!(revoked_at(first.id).await, Some(at));
    assert_eq!(revoked_at(second.id).await, Some(at));
    assert_eq!(revoked_at(earlier.id).await, Some(before));
    assert_eq!(revoked_at(others.id).await, None);
}

#[tokio::test]
async fn revoking_what_someone_holds_takes_their_bots_and_what_they_made_in_one_workspace() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "held").await;
    let (elsewhere, _) = workspace(&ctx, "held-else").await;
    let bob = ctx.create_user("held-bob").await;
    let bots = repo(&ctx);
    let adas = bots
        .create_bot(new_bot(server.id, "bot-held-ada", ada.id))
        .await
        .unwrap();
    let bobs = bots
        .create_bot(new_bot(server.id, "bot-held-bob", bob.id))
        .await
        .unwrap();
    let adas_elsewhere = bots
        .create_bot(new_bot(elsewhere.id, "bot-held-else", ada.id))
        .await
        .unwrap();
    // Ada's bot, whoever made the token; Bob's bot, with a token Ada made and one of Bob's.
    let on_adas_by_bob = bots
        .create_token(new_token(adas.user_id, "held-1", bob.id))
        .await
        .unwrap();
    let on_bobs_by_ada = bots
        .create_token(new_token(bobs.user_id, "held-2", ada.id))
        .await
        .unwrap();
    let on_bobs_by_bob = bots
        .create_token(new_token(bobs.user_id, "held-3", bob.id))
        .await
        .unwrap();
    let in_elsewhere = bots
        .create_token(new_token(adas_elsewhere.user_id, "held-4", ada.id))
        .await
        .unwrap();
    let hook_by_ada = bots
        .create_webhook(new_webhook(
            &server,
            channel_of(&ctx, &server).await,
            bobs.user_id,
            "held-5",
            ada.id,
        ))
        .await
        .unwrap();
    let hook_by_bob = bots
        .create_webhook(new_webhook(
            &server,
            channel_of(&ctx, &server).await,
            bobs.user_id,
            "held-6",
            bob.id,
        ))
        .await
        .unwrap();

    let at = Utc::now().duration_trunc(Duration::seconds(1)).unwrap();
    let revoked = bots
        .revoke_held_by(ada.id, Some(server.id), at)
        .await
        .unwrap();
    assert_eq!((revoked.tokens, revoked.webhooks), (2, 1));
    let token_revoked_at = async |id| bots.find_token(id).await.unwrap().unwrap().revoked_at;
    assert_eq!(token_revoked_at(on_adas_by_bob.id).await, Some(at));
    assert_eq!(token_revoked_at(on_bobs_by_ada.id).await, Some(at));
    assert_eq!(token_revoked_at(on_bobs_by_bob.id).await, None);
    assert_eq!(token_revoked_at(in_elsewhere.id).await, None);
    let hook_revoked_at = async |id| bots.find_webhook(id).await.unwrap().unwrap().revoked_at;
    assert_eq!(hook_revoked_at(hook_by_ada.id).await, Some(at));
    assert_eq!(hook_revoked_at(hook_by_bob.id).await, None);
    assert!(
        bots.revoke_held_by(ada.id, Some(server.id), at)
            .await
            .unwrap()
            .is_empty()
    );

    // Without a workspace, every workspace's.
    let everywhere = bots.revoke_held_by(ada.id, None, at).await.unwrap();
    assert_eq!((everywhere.tokens, everywhere.webhooks), (1, 0));
    assert_eq!(token_revoked_at(in_elsewhere.id).await, Some(at));
}

#[tokio::test]
async fn a_webhook_goes_with_its_channel() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "hook-gone").await;
    let bots = repo(&ctx);
    let bot = bots
        .create_bot(new_bot(server.id, "bot-hook-gone", ada.id))
        .await
        .unwrap();
    let channel = channel_of(&ctx, &server).await;
    let webhook = bots
        .create_webhook(new_webhook(&server, channel, bot.user_id, "gone", ada.id))
        .await
        .unwrap();

    ctx.db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM conversation WHERE id = $1",
            [Uuid::from(channel).into()],
        ))
        .await
        .unwrap();
    assert_eq!(bots.find_webhook(webhook.id).await.unwrap(), None);
}

/// A patch that turns the setting on or off and leaves the rest.
fn joins(on: bool) -> BotPatch {
    BotPatch {
        sees_member_joins: Some(on),
        ..Default::default()
    }
}

#[tokio::test]
async fn the_setting_keeps_its_start_while_on_and_forgets_it_when_off() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "setting").await;
    let bots = repo(&ctx);
    let off = bots
        .create_bot(new_bot(server.id, "bot-off", ada.id))
        .await
        .unwrap();
    assert!(!off.sees_member_joins());
    assert_eq!(off.member_joins_since, None);

    let on = bots
        .create_bot(NewBot {
            sees_member_joins: true,
            ..new_bot(server.id, "bot-on", ada.id)
        })
        .await
        .unwrap();
    assert!(on.sees_member_joins());
    assert_eq!(on.member_joins_since, Some(on.created_at));

    let renamed = bots
        .update_bot(
            on.user_id,
            BotPatch {
                name: Some("Renamed".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(renamed.member_joins_since, on.member_joins_since);

    let again = bots.update_bot(on.user_id, joins(true)).await.unwrap();
    assert_eq!(again.member_joins_since, on.member_joins_since);

    let turned_off = bots.update_bot(on.user_id, joins(false)).await.unwrap();
    assert_eq!(turned_off.member_joins_since, None);
    assert!(!turned_off.sees_member_joins());

    let turned_on = bots.update_bot(on.user_id, joins(true)).await.unwrap();
    assert!(turned_on.member_joins_since > on.member_joins_since);
}

#[tokio::test]
async fn joins_of_the_feed_keep_to_the_workspace_the_cursor_and_the_hold_back() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "feed-joins").await;
    let (other, _) = workspace(&ctx, "feed-joins-other").await;
    let bots = repo(&ctx);
    let members = SeaOrmServerMemberRepository::new(ctx.db.clone());
    members
        .add_member(server.id, ada.id, ServerMemberRole::Owner)
        .await
        .unwrap();

    let bot = bots
        .create_bot(NewBot {
            sees_member_joins: true,
            ..new_bot(server.id, "bot-feed", ada.id)
        })
        .await
        .unwrap();
    members
        .add_member(server.id, bot.user_id, ServerMemberRole::Guest)
        .await
        .unwrap();
    let start = BotFeedPosition {
        at: bots.feed_now().await.unwrap(),
        id: Uuid::nil(),
    };

    let nia = ctx.create_user("feed-joins-nia").await;
    let omar = ctx.create_user("feed-joins-omar").await;
    let elsewhere = ctx.create_user("feed-joins-elsewhere").await;
    members
        .add_member(server.id, nia.id, ServerMemberRole::Member)
        .await
        .unwrap();
    members
        .add_member(server.id, omar.id, ServerMemberRole::Member)
        .await
        .unwrap();
    members
        .add_member(other.id, elsewhere.id, ServerMemberRole::Member)
        .await
        .unwrap();

    let list = |after, hold_back, limit| {
        bots.list_feed_joins(bot.user_id, server.id, after, hold_back, limit)
    };
    assert!(
        list(start, Duration::hours(1), 10)
            .await
            .unwrap()
            .is_empty(),
        "every row is younger than the hold-back"
    );
    let found = list(start, Duration::zero(), 10).await.unwrap();
    let ids: Vec<UserId> = found.iter().map(|join| join.user_id).collect();
    assert_eq!(ids, [nia.id, omar.id]);

    let first = list(start, Duration::zero(), 1).await.unwrap();
    assert_eq!(first[0].user_id, nia.id);
    let past_first = BotFeedPosition {
        at: first[0].joined_at,
        id: first[0].member_id.into(),
    };
    let rest = list(past_first, Duration::zero(), 10).await.unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].user_id, omar.id);
}

#[tokio::test]
async fn joins_of_the_feed_start_from_when_the_bot_was_allowed_whatever_the_cursor() {
    let ctx = support::TestCtx::new().await;
    let (server, ada) = workspace(&ctx, "feed-allowed").await;
    let bots = repo(&ctx);
    let members = SeaOrmServerMemberRepository::new(ctx.db.clone());
    let bot = bots
        .create_bot(new_bot(server.id, "bot-allowed", ada.id))
        .await
        .unwrap();
    let long_ago = BotFeedPosition {
        at: Utc::now() - Duration::days(365),
        id: Uuid::nil(),
    };
    let found = || async {
        bots.list_feed_joins(bot.user_id, server.id, long_ago, Duration::zero(), 50)
            .await
            .unwrap()
            .iter()
            .map(|join| join.user_id)
            .collect::<Vec<UserId>>()
    };
    let join = |name: String| {
        let (ctx, members, server_id) = (&ctx, &members, server.id);
        async move {
            let user = ctx.create_user(&name).await;
            members
                .add_member(server_id, user.id, ServerMemberRole::Member)
                .await
                .unwrap();
            user.id
        }
    };

    // While it is off nothing comes, even with an old cursor, and a join then stays hidden.
    join("feed-allowed-before".to_string()).await;
    assert!(found().await.is_empty());
    bots.update_bot(bot.user_id, joins(true)).await.unwrap();
    assert!(found().await.is_empty());

    let during = join("feed-allowed-during".to_string()).await;
    assert_eq!(found().await, [during]);

    // Turning it on twice doesn't move the start.
    bots.update_bot(bot.user_id, joins(true)).await.unwrap();
    assert_eq!(found().await, [during]);

    // Off stops at once, and on again doesn't reveal the gap between.
    bots.update_bot(bot.user_id, joins(false)).await.unwrap();
    assert!(found().await.is_empty());
    join("feed-allowed-gap".to_string()).await;
    bots.update_bot(bot.user_id, joins(true)).await.unwrap();
    assert!(found().await.is_empty());

    let after = join("feed-allowed-after".to_string()).await;
    assert_eq!(found().await, [after]);
}
