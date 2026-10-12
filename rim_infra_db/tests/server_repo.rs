#![cfg(feature = "db-tests")]

mod support;

use rim_core::server::repository::ServerRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;

use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;

#[tokio::test]
async fn create_and_get() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmServerRepository::new(ctx.db.clone());
    let created = repo.create_server("s_test").await.unwrap();
    let fetched = repo.get_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.name, "s_test");
}

#[tokio::test]
async fn get_servers_for_user_returns_joined_servers() {
    let ctx = support::TestCtx::new().await;

    let server_repo = SeaOrmServerRepository::new(ctx.db.clone());
    let member_repo = SeaOrmServerMemberRepository::new(ctx.db.clone());

    let user = ctx.create_user("u").await;
    let s1 = server_repo.create_server("s1").await.unwrap();
    let s2 = server_repo.create_server("s2").await.unwrap();

    assert_eq!(
        server_repo
            .get_servers_for_user(user.id)
            .await
            .unwrap()
            .len(),
        0
    );

    let _ = member_repo
        .add_member(s1.id, user.id, ServerMemberRole::Member)
        .await
        .unwrap();
    let _ = member_repo
        .add_member(s2.id, user.id, ServerMemberRole::Owner)
        .await
        .unwrap();

    let mut servers = server_repo.get_servers_for_user(user.id).await.unwrap();
    servers.sort_by_key(|s| s.name.clone());
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0].id, s1.id);
    assert_eq!(servers[1].id, s2.id);
}

#[tokio::test]
async fn find_owned_by_name_matches_the_owner_ignoring_case_and_spaces() {
    let ctx = support::TestCtx::new().await;

    let server_repo = SeaOrmServerRepository::new(ctx.db.clone());
    let member_repo = SeaOrmServerMemberRepository::new(ctx.db.clone());

    let owner = ctx.create_user("owner").await;
    let member = ctx.create_user("member").await;
    let design = server_repo.create_server("Design").await.unwrap();
    member_repo
        .add_member(design.id, owner.id, ServerMemberRole::Owner)
        .await
        .unwrap();
    member_repo
        .add_member(design.id, member.id, ServerMemberRole::Moderator)
        .await
        .unwrap();

    let found = server_repo
        .find_owned_by_name(owner.id, "  dESIGN ")
        .await
        .unwrap()
        .expect("the owner's workspace");
    assert_eq!((found.id, found.name.as_str()), (design.id, "Design"));

    assert!(
        server_repo
            .find_owned_by_name(owner.id, "Design team")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        server_repo
            .find_owned_by_name(member.id, "Design")
            .await
            .unwrap()
            .is_none()
    );
}

/// The first column of the first row `sql` answers, as a count.
async fn count(ctx: &support::TestCtx, sql: &str) -> i64 {
    use sea_orm::ConnectionTrait;
    ctx.db
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Postgres,
            sql.to_string(),
        ))
        .await
        .expect(sql)
        .expect("a row")
        .try_get_by_index::<i64>(0)
        .expect("count")
}

#[tokio::test]
async fn delete_workspaces_removes_everything_in_them_returns_their_files_and_spares_the_direct_space()
 {
    use sea_orm::ConnectionTrait;

    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmServerRepository::new(ctx.db.clone());
    let owner = ctx.create_user("owner").await;
    let doomed = repo.create_server("Doomed").await.unwrap();
    let kept = repo.create_server("Kept").await.unwrap();
    let members = SeaOrmServerMemberRepository::new(ctx.db.clone());
    members
        .add_member(doomed.id, owner.id, ServerMemberRole::Owner)
        .await
        .unwrap();
    let channel = ctx
        .create_conversation(
            doomed.id,
            rim_core::conversation::model::ConversationType::Channel,
        )
        .await;
    let message = ctx.create_message(channel.id, owner.id, "hello").await;
    ctx.db
        .execute_unprepared(&format!(
            "INSERT INTO message_attachment (id, storage_url, message_id, file_size, file_name, \
             mime_type) VALUES (gen_random_uuid(), 'uploads/a.png', '{message}', 1, 'a.png', \
             'image/png'), (gen_random_uuid(), 'pending://t', '{message}', 1, 'b.png', \
             'image/png'); \
             INSERT INTO custom_emoji (id, server_id, name, image_url, uploaded_by) \
             VALUES (gen_random_uuid(), '{doomed}', 'wave', 'emoji/wave.png', '{owner}'); \
             INSERT INTO keybind (id, scope, subject_id, keystroke, command_id, context) VALUES \
             (gen_random_uuid(), 'workspace', '{doomed}', 'Ctrl+K', 'x', 'global'), \
             (gen_random_uuid(), 'workspace', '{kept}', 'Ctrl+K', 'x', 'global'); \
             UPDATE server SET kind = 'direct' WHERE id = '{kept}';",
            message = message.id,
            doomed = doomed.id,
            kept = kept.id,
            owner = owner.id,
        ))
        .await
        .unwrap();

    let deleted = repo.delete_workspaces(&[doomed.id, kept.id]).await.unwrap();

    assert_eq!(deleted.deleted, 1);
    let mut keys = deleted.storage_keys;
    keys.sort();
    assert_eq!(keys, vec!["emoji/wave.png", "uploads/a.png"]);
    assert!(repo.get_by_id(doomed.id).await.unwrap().is_none());
    assert!(repo.get_by_id(kept.id).await.unwrap().is_some());
    for (table, n) in [
        ("server_member", 0),
        ("conversation", 0),
        ("message", 0),
        ("message_attachment", 0),
        ("custom_emoji", 0),
        ("keybind", 1),
    ] {
        assert_eq!(
            count(&ctx, &format!("SELECT count(*) FROM {table}")).await,
            n,
            "{table}"
        );
    }
}
