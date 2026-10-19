#![cfg(feature = "db-tests")]

mod support;

use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;

#[tokio::test]
async fn create_and_get_user_roundtrip() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmUserRepository::new(ctx.db.clone());
    let created = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_roundtrip".to_string(),
            password: Some("pw".to_string()),
            email: Some("u_test_roundtrip@test.example.com".to_string()),
            display_name: None,
            key_identity: None,
        })
        .await
        .unwrap();

    let fetched = repo.get_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.username, "u_test_roundtrip");
    assert_eq!(
        fetched.email.as_deref(),
        Some("u_test_roundtrip@test.example.com")
    );
}

#[tokio::test]
async fn create_user_duplicate_username_is_conflict() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmUserRepository::new(ctx.db.clone());

    // Schema enforces username uniqueness via a unique index.
    let _u1 = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_duplicate".to_string(),
            password: Some("pw".to_string()),
            email: Some("u_test_duplicate@test.example.com".to_string()),
            display_name: None,
            key_identity: None,
        })
        .await
        .unwrap();

    let err = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_duplicate".to_string(),
            password: Some("pw2".to_string()),
            email: Some("u_test_duplicate2@test.example.com".to_string()),
            display_name: None,
            key_identity: None,
        })
        .await
        .unwrap_err();

    assert!(matches!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::Conflict
    ));
}

#[tokio::test]
async fn find_by_username() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmUserRepository::new(ctx.db.clone());

    assert!(repo.find_by_username("missing").await.unwrap().is_none());

    let created = repo
        .create_user(rim_core::user::model::NewUser {
            username: "u_test_find".to_string(),
            password: Some("pw".to_string()),
            email: Some("u_test_find@test.example.com".to_string()),
            display_name: None,
            key_identity: None,
        })
        .await
        .unwrap();

    let found = repo.find_by_username("u_test_find").await.unwrap().unwrap();
    assert_eq!(found.id, created.id);
}

#[tokio::test]
async fn find_by_username_ignores_case() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmUserRepository::new(ctx.db.clone());
    let created = ctx.create_user("u_test_case").await;

    for typed in ["U_Test_Case", "U_TEST_CASE", "u_test_case"] {
        let found = repo.find_by_username(typed).await.unwrap().unwrap();
        assert_eq!(found.id, created.id, "{typed}");
    }
    assert!(repo.find_by_username("u_test_cas").await.unwrap().is_none());
}

#[tokio::test]
async fn a_username_differing_only_in_case_is_conflict() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmUserRepository::new(ctx.db.clone());
    ctx.create_user("u_test_fold").await;

    let err = repo
        .create_user(rim_core::user::model::NewUser {
            username: "U_Test_Fold".to_string(),
            password: Some("pw".to_string()),
            email: Some("u_test_fold2@test.example.com".to_string()),
            display_name: None,
            key_identity: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::Conflict
    ));
}

#[tokio::test]
async fn lock_user_reports_whether_the_user_exists() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmUserRepository::new(ctx.db.clone());
    let alice = ctx.create_user("u_test_lock").await;

    assert!(repo.lock_user(alice.id).await.unwrap());
    assert!(
        !repo
            .lock_user(rim_core::user::model::UserId::new())
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn record_terms_acceptance_stores_the_url_and_time() {
    use sea_orm::EntityTrait;

    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmUserRepository::new(ctx.db.clone());
    let user = ctx.create_user("u_test_terms").await;
    let untouched = ctx.create_user("u_test_no_terms").await;
    let at = chrono::DateTime::parse_from_rfc3339("2026-10-12T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    repo.record_terms_acceptance(user.id, "https://rim.example.org/terms".to_string(), at)
        .await
        .unwrap();

    let row = entity::user::Entity::find_by_id(sea_orm::prelude::Uuid::from(user.id))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.terms_accepted_url.as_deref(),
        Some("https://rim.example.org/terms")
    );
    assert_eq!(row.terms_accepted_at.map(|t| t.to_utc()), Some(at));
    let row = entity::user::Entity::find_by_id(sea_orm::prelude::Uuid::from(untouched.id))
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.terms_accepted_url, row.terms_accepted_at),
        (None, None)
    );

    let err = repo
        .record_terms_acceptance(
            rim_core::user::model::UserId::new(),
            "https://rim.example.org/terms".to_string(),
            at,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::NotFound
    ));
}

#[tokio::test]
async fn the_invite_chain_is_recorded_and_an_accounts_invites_are_listed_and_revoked() {
    use chrono::{Duration, Utc};
    use rim_core::auth::model::{NewInstanceInvite, NewInvite};
    use rim_core::auth::repository::{InstanceInviteRepository, InviteRepository};
    use rim_core::server_member::model::ServerMemberRole;
    use rim_core::user::model::{InviteChain, InviteKind};
    use rim_infra_db::sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository;
    use rim_infra_db::sea_orm_impl::invite::SeaOrmInviteRepository;

    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmUserRepository::new(ctx.db.clone());
    let instance_invites = SeaOrmInstanceInviteRepository::new(ctx.db.clone());
    let workspace_invites = SeaOrmInviteRepository::new(ctx.db.clone());
    let ada = ctx.create_user("u_chain_ada").await;
    let bob = ctx.create_user("u_chain_bob").await;
    let cy = ctx.create_user("u_chain_cy").await;
    let server = ctx.create_server("chain").await;

    let instance = |max_uses| NewInstanceInvite {
        created_by_user_id: Some(ada.id),
        token_hash: format!("hash-{}", uuid::Uuid::now_v7()),
        expires_at: Utc::now() + Duration::days(1),
        max_uses,
        email: None,
    };
    let live = instance_invites.create(instance(2)).await.unwrap();
    let used_up = instance_invites.create(instance(1)).await.unwrap();
    assert!(instance_invites.try_consume(used_up.id).await.unwrap());
    let revoked = instance_invites.create(instance(3)).await.unwrap();
    instance_invites.revoke(revoked.id).await.unwrap();
    let workspace = workspace_invites
        .create_invite(NewInvite {
            server_id: server.id,
            created_by_user_id: ada.id,
            token_hash: format!("hash-{}", uuid::Uuid::now_v7()),
            expires_at: None,
            max_uses: 5,
            role: ServerMemberRole::Member,
        })
        .await
        .unwrap();
    // Someone else's invite is never touched.
    let others = instance_invites
        .create(NewInstanceInvite {
            created_by_user_id: Some(cy.id),
            ..instance(1)
        })
        .await
        .unwrap();

    repo.record_invite_chain(bob.id, used_up.id.into(), Some(ada.id))
        .await
        .unwrap();
    repo.record_invite_chain(cy.id, workspace.id.into(), Some(ada.id))
        .await
        .unwrap();
    assert_eq!(
        repo.invite_chain(bob.id).await.unwrap(),
        Some(InviteChain {
            invite_id: used_up.id.into(),
            invited_by_user_id: Some(ada.id),
        })
    );
    assert_eq!(repo.invite_chain(ada.id).await.unwrap(), None);
    assert_eq!(
        repo.invited_users(ada.id).await.unwrap(),
        vec![bob.id, cy.id]
    );
    assert!(repo.invited_users(bob.id).await.unwrap().is_empty());

    let listed = repo.invites_created_by(ada.id).await.unwrap();
    let mut kinds: Vec<(uuid::Uuid, InviteKind)> = listed.iter().map(|i| (i.id, i.kind)).collect();
    kinds.sort_by_key(|(id, _)| *id);
    let mut expected = vec![
        (live.id.into(), InviteKind::Instance),
        (used_up.id.into(), InviteKind::Instance),
        (revoked.id.into(), InviteKind::Instance),
        (workspace.id.into(), InviteKind::Workspace),
    ];
    expected.sort_by_key(|(id, _)| *id);
    assert_eq!(kinds, expected);
    assert!(
        listed
            .windows(2)
            .all(|pair| pair[0].created_at >= pair[1].created_at),
        "newest first"
    );

    // The live instance invite and the workspace invite have uses left.
    assert_eq!(repo.revoke_invites_created_by(ada.id).await.unwrap(), 2);
    assert_eq!(repo.revoke_invites_created_by(ada.id).await.unwrap(), 0);
    let after = repo.invites_created_by(ada.id).await.unwrap();
    assert!(
        after
            .iter()
            .all(|i| i.revoked_at.is_some() || i.id == uuid::Uuid::from(used_up.id))
    );
    assert!(
        instance_invites
            .get_by_id(others.id)
            .await
            .unwrap()
            .unwrap()
            .revoked_at
            .is_none()
    );

    let err = repo
        .record_invite_chain(
            rim_core::user::model::UserId::new(),
            live.id.into(),
            Some(ada.id),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::NotFound
    ));
}

#[tokio::test]
async fn a_blocked_email_is_matched_case_insensitively_until_unblocked() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmUserRepository::new(ctx.db.clone());

    assert!(!repo.is_email_blocked("spam@example.com").await.unwrap());
    assert!(repo.block_email("Spam@Example.com", "spam").await.unwrap());
    assert!(
        !repo
            .block_email("spam@example.com", "spam wave")
            .await
            .unwrap()
    );
    assert!(repo.is_email_blocked("SPAM@example.COM").await.unwrap());
    assert!(!repo.is_email_blocked("other@example.com").await.unwrap());

    assert!(repo.unblock_email("spam@EXAMPLE.com").await.unwrap());
    assert!(!repo.unblock_email("spam@example.com").await.unwrap());
    assert!(!repo.is_email_blocked("spam@example.com").await.unwrap());
}

#[tokio::test]
async fn a_purged_accounts_email_is_free_unless_it_was_blocked() {
    use sea_orm::ConnectionTrait;

    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmUserRepository::new(ctx.db.clone());
    let kept = ctx.create_user("u_purge_kept").await;
    let banned = ctx.create_user("u_purge_banned").await;
    let kept_email = kept.email.clone().unwrap();
    let banned_email = banned.email.clone().unwrap();
    repo.block_email(&banned_email, "spam").await.unwrap();
    // What the purge leaves: a tombstone without an email.
    for user in [&kept, &banned] {
        ctx.db
            .execute_raw(sea_orm::Statement::from_sql_and_values(
                sea_orm::DbBackend::Postgres,
                r#"UPDATE "user" SET deleted_at = now(), email = NULL WHERE id = $1"#,
                [sea_orm::prelude::Uuid::from(user.id).into()],
            ))
            .await
            .unwrap();
    }

    assert!(repo.find_by_email(&kept_email).await.unwrap().is_none());
    assert!(!repo.is_email_blocked(&kept_email).await.unwrap());
    assert!(repo.find_by_email(&banned_email).await.unwrap().is_none());
    assert!(repo.is_email_blocked(&banned_email).await.unwrap());
}

#[tokio::test]
async fn the_instance_operator_role_is_granted_listed_and_revoked_and_a_tombstone_never_holds_it() {
    use sea_orm::ConnectionTrait;

    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmUserRepository::new(ctx.db.clone());
    let zed = ctx.create_user("u_operator_zed").await;
    let amy = ctx.create_user("u_operator_amy").await;
    let plain = ctx.create_user("u_operator_plain").await;
    assert!(!repo.is_instance_operator(zed.id).await.unwrap());

    for user in [&zed, &amy] {
        assert!(repo.set_instance_operator(user.id, true).await.unwrap());
    }
    assert!(!repo.set_instance_operator(zed.id, true).await.unwrap());
    assert!(repo.is_instance_operator(zed.id).await.unwrap());
    assert!(!repo.is_instance_operator(plain.id).await.unwrap());
    let names: Vec<String> = repo
        .instance_operators()
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.username)
        .collect();
    assert_eq!(names, vec!["u_operator_amy", "u_operator_zed"]);

    assert!(repo.set_instance_operator(zed.id, false).await.unwrap());
    assert!(!repo.set_instance_operator(zed.id, false).await.unwrap());
    assert!(!repo.is_instance_operator(zed.id).await.unwrap());

    ctx.db
        .execute_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            r#"UPDATE "user" SET deleted_at = now(), email = NULL WHERE id = $1"#,
            [sea_orm::prelude::Uuid::from(amy.id).into()],
        ))
        .await
        .unwrap();
    assert!(!repo.is_instance_operator(amy.id).await.unwrap());
    assert!(repo.instance_operators().await.unwrap().is_empty());
}
