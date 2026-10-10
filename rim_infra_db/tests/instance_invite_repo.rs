#![cfg(feature = "db-tests")]

mod support;

use chrono::{Duration, Utc};
use rim_core::auth::model::NewInstanceInvite;
use rim_core::auth::repository::InstanceInviteRepository;
use rim_infra_db::sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository;

#[tokio::test]
async fn an_operator_invite_has_no_creating_user_and_is_consumed_listed_and_revoked() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmInstanceInviteRepository::new(ctx.db.clone());

    let operator = repo
        .create(NewInstanceInvite {
            created_by_user_id: None,
            token_hash: "operator-hash".to_string(),
            expires_at: Utc::now() + Duration::days(1),
            max_uses: 1,
            email: Some("ops@example.com".to_string()),
        })
        .await
        .unwrap();
    assert_eq!(operator.created_by_user_id, None);
    assert_eq!(operator.email.as_deref(), Some("ops@example.com"));

    let user = ctx.create_user("issuer").await;
    let issued = repo
        .create(NewInstanceInvite {
            created_by_user_id: Some(user.id),
            token_hash: "user-hash".to_string(),
            expires_at: Utc::now() + Duration::days(1),
            max_uses: 2,
            email: None,
        })
        .await
        .unwrap();
    assert_eq!(issued.created_by_user_id, Some(user.id));

    let found = repo
        .get_by_token_hash("operator-hash")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, operator.id);
    assert_eq!(found.created_by_user_id, None);
    assert_eq!(found.email.as_deref(), Some("ops@example.com"));
    assert!(repo.try_consume(operator.id).await.unwrap());
    assert!(!repo.try_consume(operator.id).await.unwrap());

    let listed = repo.list_active().await.unwrap();
    assert!(
        listed
            .iter()
            .any(|i| i.id == operator.id && i.use_count == 1)
    );
    assert!(listed.iter().any(|i| i.id == issued.id));

    repo.revoke(operator.id).await.unwrap();
    let listed = repo.list_active().await.unwrap();
    assert!(listed.iter().all(|i| i.id != operator.id));
}
