#![cfg(feature = "db-tests")]

//! Account passwords, login states, rewraps and resets against a live Postgres.
//! Destructive for the shared test database: run single-threaded.

mod support;

use chrono::{Duration, SubsecRound, Utc};

use rim_core::account_password::model::{
    AttemptSchedule, AttemptStart, PasswordLogin, PasswordLoginId, PasswordLoginPurpose,
    PasswordReset, PasswordResetId,
};
use rim_core::account_password::repository::AccountPasswordRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_infra_db::sea_orm_impl::account_password::SeaOrmAccountPasswordRepository;
use sea_orm::ConnectionTrait;

#[tokio::test]
async fn a_password_is_stored_once_replaced_counted_and_goes_with_its_account() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmAccountPasswordRepository::new(ctx.db.clone());
    let user = ctx.create_user("ada").await;
    let now = Utc::now().trunc_subsecs(6);

    assert_eq!(repo.get(user.id).await.unwrap(), None);
    assert_eq!(repo.count_passwords().await.unwrap(), 0);
    repo.insert(user.id, vec![1, 2], vec![3, 4], now)
        .await
        .unwrap();
    let err = repo
        .insert(user.id, vec![9], vec![9], now)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::Conflict);
    let stored = repo.get(user.id).await.unwrap().unwrap();
    assert_eq!(stored.registration, vec![1, 2]);
    assert_eq!(stored.share_sealed, vec![3, 4]);
    assert_eq!(stored.failed_attempts, 0);
    assert_eq!(stored.created_at, now);
    assert_eq!(repo.count_passwords().await.unwrap(), 1);

    let schedule = AttemptSchedule::default();
    for _ in 0..4 {
        repo.start_attempt(user.id, now, &schedule).await.unwrap();
    }
    assert!(
        repo.replace_registration(user.id, vec![5], now)
            .await
            .unwrap()
    );
    let replaced = repo.get(user.id).await.unwrap().unwrap();
    assert_eq!(replaced.registration, vec![5]);
    assert_eq!(replaced.share_sealed, vec![3, 4], "the share stays");
    assert_eq!(replaced.failed_attempts, 0);
    assert_eq!(replaced.next_attempt_at, None);
    let stranger = ctx.create_user("bob").await;
    assert!(
        !repo
            .replace_registration(stranger.id, vec![5], now)
            .await
            .unwrap()
    );

    ctx.db
        .execute_unprepared(&format!("DELETE FROM \"user\" WHERE id = '{}'", user.id))
        .await
        .unwrap();
    assert_eq!(repo.count_passwords().await.unwrap(), 0);
}

#[tokio::test]
async fn tries_follow_the_schedule_in_one_statement_and_the_tenth_locks() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmAccountPasswordRepository::new(ctx.db.clone());
    let user = ctx.create_user("ada").await;
    let schedule = AttemptSchedule::default();
    let mut now = Utc::now().trunc_subsecs(6);

    let nobody = ctx.create_user("nobody").await;
    assert_eq!(
        repo.start_attempt(nobody.id, now, &schedule).await.unwrap(),
        AttemptStart::NoPassword
    );

    repo.insert(user.id, vec![7], vec![8], now).await.unwrap();
    for count in 1..=3u32 {
        assert_eq!(
            repo.start_attempt(user.id, now, &schedule).await.unwrap(),
            AttemptStart::Counted {
                registration: vec![7],
                failed_attempts: count
            }
        );
    }
    for count in 4..=9u32 {
        let started = repo.start_attempt(user.id, now, &schedule).await.unwrap();
        assert!(
            matches!(started, AttemptStart::Counted { failed_attempts, .. } if failed_attempts == count),
            "{started:?}"
        );
        let wait = schedule.wait_after(count).unwrap();
        assert_eq!(
            repo.start_attempt(user.id, now, &schedule).await.unwrap(),
            AttemptStart::Wait { until: now + wait }
        );
        now += wait;
    }
    let tenth = repo.start_attempt(user.id, now, &schedule).await.unwrap();
    assert!(
        matches!(
            tenth,
            AttemptStart::Counted {
                failed_attempts: 10,
                ..
            }
        ),
        "{tenth:?}"
    );
    let locked = repo.get(user.id).await.unwrap().unwrap();
    assert_eq!(locked.locked_at, Some(now));
    assert_eq!(locked.next_attempt_at, None);
    assert_eq!(
        repo.start_attempt(user.id, now + Duration::days(9), &schedule)
            .await
            .unwrap(),
        AttemptStart::Locked
    );

    repo.clear_attempts(user.id, now).await.unwrap();
    let cleared = repo.get(user.id).await.unwrap().unwrap();
    assert_eq!(cleared.failed_attempts, 0);
    assert_eq!(cleared.locked_at, None);
}

#[tokio::test]
async fn a_login_state_is_taken_once_and_expired_ones_go_on_insert() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmAccountPasswordRepository::new(ctx.db.clone());
    let user = ctx.create_user("ada").await;
    let now = Utc::now().trunc_subsecs(6);
    let old = PasswordLogin {
        id: PasswordLoginId::new(),
        user_id: Some(user.id),
        purpose: PasswordLoginPurpose::Login,
        state: vec![1],
        expires_at: now - Duration::seconds(1),
    };
    repo.insert_login(old.clone(), now - Duration::minutes(5))
        .await
        .unwrap();
    let fresh = PasswordLogin {
        id: PasswordLoginId::new(),
        user_id: Some(user.id),
        purpose: PasswordLoginPurpose::Change,
        state: vec![2, 3],
        expires_at: now + Duration::seconds(120),
    };
    repo.insert_login(fresh.clone(), now).await.unwrap();
    assert_eq!(repo.take_login(old.id).await.unwrap(), None);
    assert_eq!(
        repo.take_login(fresh.id).await.unwrap(),
        Some(fresh.clone())
    );
    assert_eq!(repo.take_login(fresh.id).await.unwrap(), None);
}

#[tokio::test]
async fn rewraps_are_listed_newest_first_and_pruned_to_the_newest_three() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmAccountPasswordRepository::new(ctx.db.clone());
    let user = ctx.create_user("ada").await;
    let other = ctx.create_user("bob").await;
    let now = Utc::now().trunc_subsecs(6);
    for i in 0..5u8 {
        repo.insert_rewrap(user.id, vec![i], now + Duration::seconds(i64::from(i)), 3)
            .await
            .unwrap();
    }
    repo.insert_rewrap(other.id, vec![99], now, 3)
        .await
        .unwrap();
    assert_eq!(
        repo.list_rewraps(user.id).await.unwrap(),
        vec![vec![4], vec![3], vec![2]]
    );
    assert_eq!(repo.delete_rewraps(user.id).await.unwrap(), 3);
    assert!(repo.list_rewraps(user.id).await.unwrap().is_empty());
    assert_eq!(repo.list_rewraps(other.id).await.unwrap(), vec![vec![99]]);
}

#[tokio::test]
async fn a_reset_is_taken_once() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmAccountPasswordRepository::new(ctx.db.clone());
    let user = ctx.create_user("ada").await;
    let now = Utc::now().trunc_subsecs(6);
    let reset = PasswordReset {
        id: PasswordResetId::new(),
        user_id: user.id,
        expires_at: now + Duration::seconds(120),
    };
    repo.insert_reset(reset.clone(), now).await.unwrap();
    assert_eq!(
        repo.take_reset(reset.id).await.unwrap(),
        Some(reset.clone())
    );
    assert_eq!(repo.take_reset(reset.id).await.unwrap(), None);
}

#[tokio::test]
async fn a_ticket_is_consumed_once_only_by_its_account_before_it_expires() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmAccountPasswordRepository::new(ctx.db.clone());
    let ada = ctx.create_user("ada").await;
    let bob = ctx.create_user("bob").await;
    let now = Utc::now().trunc_subsecs(6);
    let later = now + Duration::seconds(120);
    repo.insert_ticket(
        "expired",
        ada.id,
        now - Duration::seconds(1),
        now - Duration::minutes(5),
    )
    .await
    .unwrap();
    repo.insert_ticket("live", ada.id, later, now)
        .await
        .unwrap();
    // Inserting deleted the expired ticket.
    assert!(
        !repo
            .consume_ticket("expired", ada.id, now - Duration::minutes(5))
            .await
            .unwrap()
    );
    assert!(!repo.consume_ticket("live", bob.id, now).await.unwrap());
    assert!(!repo.consume_ticket("live", ada.id, later).await.unwrap());
    assert!(repo.consume_ticket("live", ada.id, now).await.unwrap());
    assert!(!repo.consume_ticket("live", ada.id, now).await.unwrap());
}
