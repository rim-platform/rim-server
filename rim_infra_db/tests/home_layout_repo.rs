#![cfg(feature = "db-tests")]

mod support;

use std::sync::Arc;

use chrono::{DateTime, Duration, DurationRound, Utc};
use migration::{Migrator, MigratorTrait};
use rim_core::account_deletion::repository::AccountDeletionRepository;
use rim_core::home_layout::repository::HomeLayoutRepository;
use rim_core::host::model::UserKind;
use rim_core::user::model::UserId;
use rim_infra_db::sea_orm_impl::account_deletion::SeaOrmAccountDeletionRepository;
use rim_infra_db::sea_orm_impl::home_layout::SeaOrmHomeLayoutRepository;
use sea_orm::prelude::Uuid;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::json;

const MIGRATION_NAME: &str = "m20261107_151959_create_home_layout";

fn repo(ctx: &support::TestCtx) -> SeaOrmHomeLayoutRepository {
    SeaOrmHomeLayoutRepository::new(ctx.db.clone())
}

fn layout(id: &str) -> serde_json::Value {
    json!({ "version": 1, "boxes": [{ "id": id, "kind": "waiting", "width": "half" }] })
}

/// A time Postgres stores without losing precision.
fn at(minutes_ago: i64) -> DateTime<Utc> {
    (Utc::now() - Duration::minutes(minutes_ago))
        .duration_trunc(Duration::microseconds(1))
        .unwrap()
}

async fn sql(ctx: &support::TestCtx, statement: &str) {
    ctx.db
        .execute_raw(Statement::from_string(DbBackend::Postgres, statement))
        .await
        .unwrap();
}

#[tokio::test]
async fn nothing_is_stored_until_a_save() {
    let ctx = support::TestCtx::new().await;
    let ada = ctx.create_user("ada").await;
    assert!(repo(&ctx).get(ada.id).await.unwrap().is_none());
    assert!(!repo(&ctx).delete(ada.id).await.unwrap());
}

#[tokio::test]
async fn a_newer_or_equal_edit_replaces_the_layout_and_an_older_one_does_not() {
    let ctx = support::TestCtx::new().await;
    let ada = ctx.create_user("ada").await;
    let repo = repo(&ctx);

    let (ten, eight, five) = (at(10), at(8), at(5));

    let first = repo
        .save_unless_newer(ada.id, layout("first"), ten)
        .await
        .unwrap()
        .expect("the first save is stored");
    assert_eq!(first.layout, layout("first"));
    assert_eq!(first.edited_at, ten);

    let newer = repo
        .save_unless_newer(ada.id, layout("newer"), five)
        .await
        .unwrap()
        .expect("a newer edit is stored");
    assert_eq!(newer.layout, layout("newer"));
    assert_eq!(newer.edited_at, five);

    assert!(
        repo.save_unless_newer(ada.id, layout("older"), eight)
            .await
            .unwrap()
            .is_none(),
        "an older edit keeps the stored layout"
    );
    let kept = repo.get(ada.id).await.unwrap().expect("stored");
    assert_eq!(kept.layout, layout("newer"));
    assert_eq!(kept.edited_at, newer.edited_at);

    let equal = repo
        .save_unless_newer(ada.id, layout("equal"), newer.edited_at)
        .await
        .unwrap()
        .expect("an edit made at the same time is stored");
    assert_eq!(equal.layout, layout("equal"));
    assert!(equal.updated_at >= newer.updated_at);
}

#[tokio::test]
async fn two_saves_at_once_keep_the_later_edit_and_neither_fails() {
    let ctx = support::TestCtx::new().await;
    let ada = ctx.create_user("ada").await;
    let repo = Arc::new(repo(&ctx));
    for round in 0..20 {
        if round % 2 == 0 {
            repo.delete(ada.id).await.unwrap();
        }
        let (early, late) = (at(30 - round), at(10 - round));
        let (first, second) = if round % 4 < 2 {
            ((layout("late"), late), (layout("early"), early))
        } else {
            ((layout("early"), early), (layout("late"), late))
        };
        let a = tokio::spawn({
            let repo = Arc::clone(&repo);
            async move { repo.save_unless_newer(ada.id, first.0, first.1).await }
        });
        let b = tokio::spawn({
            let repo = Arc::clone(&repo);
            async move { repo.save_unless_newer(ada.id, second.0, second.1).await }
        });
        a.await.unwrap().expect("no error");
        b.await.unwrap().expect("no error");
        let stored = repo.get(ada.id).await.unwrap().expect("stored");
        assert_eq!(stored.layout, layout("late"), "round {round}");
        assert_eq!(stored.edited_at, late, "round {round}");
    }
}

#[tokio::test]
async fn each_account_has_its_own_layout() {
    let ctx = support::TestCtx::new().await;
    let ada = ctx.create_user("ada").await;
    let bob = ctx.create_user("bob").await;
    let repo = repo(&ctx);
    repo.save_unless_newer(ada.id, layout("ada"), at(1))
        .await
        .unwrap();
    assert!(repo.get(bob.id).await.unwrap().is_none());
    assert!(!repo.delete(bob.id).await.unwrap());
    assert_eq!(
        repo.get(ada.id).await.unwrap().expect("stored").layout,
        layout("ada")
    );
    assert!(repo.delete(ada.id).await.unwrap());
    assert!(repo.get(ada.id).await.unwrap().is_none());
}

#[tokio::test]
async fn the_kind_of_account_is_read_from_its_user_row() {
    let ctx = support::TestCtx::new().await;
    let ada = ctx.create_user("ada").await;
    let remote = ctx.create_user("remote").await;
    sql(
        &ctx,
        &format!(
            r#"UPDATE "user" SET kind = 'remote' WHERE id = '{}'"#,
            remote.id
        ),
    )
    .await;
    let repo = repo(&ctx);
    assert_eq!(
        repo.account_kind(ada.id).await.unwrap(),
        Some(UserKind::Local)
    );
    assert_eq!(
        repo.account_kind(remote.id).await.unwrap(),
        Some(UserKind::Remote)
    );
    assert_eq!(repo.account_kind(UserId::new()).await.unwrap(), None);
}

#[tokio::test]
async fn the_purge_deletes_the_layout_and_deleting_the_user_row_cascades() {
    let ctx = support::TestCtx::new().await;
    let ada = ctx.create_user("ada").await;
    let bob = ctx.create_user("bob").await;
    let repo = repo(&ctx);
    repo.save_unless_newer(ada.id, layout("ada"), at(1))
        .await
        .unwrap();
    repo.save_unless_newer(bob.id, layout("bob"), at(1))
        .await
        .unwrap();

    let personal = SeaOrmAccountDeletionRepository::new(ctx.db.clone())
        .purge_personal_data(ada.id)
        .await
        .unwrap();
    assert!(personal.counts.contains(&("home_layout", 1)));
    assert!(repo.get(ada.id).await.unwrap().is_none());
    assert!(repo.get(bob.id).await.unwrap().is_some());

    ctx.db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"DELETE FROM "user" WHERE id = $1"#,
            [Uuid::from(bob.id).into()],
        ))
        .await
        .unwrap();
    assert!(repo.get(bob.id).await.unwrap().is_none());
}

#[tokio::test]
async fn the_migration_goes_down_and_up() {
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
    let ada = ctx.create_user("ada").await;
    repo(&ctx)
        .save_unless_newer(ada.id, layout("ada"), at(1))
        .await
        .unwrap();

    let conn = support::db_conn().await;
    support::roll_back(&conn, steps).await.unwrap();
    let gone = conn
        .execute_unprepared("SELECT 1 FROM home_layout")
        .await
        .unwrap_err();
    assert!(gone.to_string().contains("home_layout"), "{gone}");

    support::migrate_up(&conn).await.unwrap();
    assert!(repo(&ctx).get(ada.id).await.unwrap().is_none());
    repo(&ctx)
        .save_unless_newer(ada.id, layout("again"), at(1))
        .await
        .unwrap()
        .expect("stored again");
}
