#![cfg(feature = "db-tests")]

//! Workspace DM rules and each user's choices against a real database: the defaults before
//! anyone chose, the rules only an existing workspace has, a user's workspaces with their
//! choice, the workspaces two people share with the sender's role, and the migrations' way
//! back.

mod support;

use migration::{Migrator, MigratorTrait};
use rim_core::dm_policy::model::{MemberDm, OutsideDm, OutsideFloor};
use rim_core::dm_policy::repository::DmPolicyRepository;
use rim_core::host::repository::HostRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_infra_db::sea_orm_impl::dm_policy::SeaOrmDmPolicyRepository;
use rim_infra_db::sea_orm_impl::host::SeaOrmHostRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;
use sea_orm::{ConnectionTrait, DbBackend, Statement};

const FIRST_MIGRATION: &str = "m20261016_223549_add_workspace_dm_policy";

async fn exists(db: &sea_orm::DatabaseConnection, sql: &str) -> bool {
    db.query_one_raw(Statement::from_string(DbBackend::Postgres, sql))
        .await
        .unwrap()
        .unwrap()
        .try_get::<bool>("", "present")
        .unwrap()
}

/// Ada (owner) and Bob (moderator) in Acme, Ada and Carol (member) in Beta, and all three
/// guests of the direct space.
struct People {
    ctx: support::TestCtx,
    repo: SeaOrmDmPolicyRepository,
    ada: rim_core::user::model::User,
    acme: ServerId,
    beta: ServerId,
    direct: ServerId,
}

async fn people() -> People {
    let ctx = support::TestCtx::new().await;
    let ada = ctx.create_key_only_user("ada", "rimp1_ada").await;
    let bob = ctx.create_key_only_user("bob", "rimp1_bob").await;
    let carol = ctx.create_key_only_user("carol", "rimp1_carol").await;
    let members = SeaOrmServerMemberRepository::new(ctx.db.clone());
    let acme = ctx.create_server("Acme").await.id;
    let beta = ctx.create_server("Beta").await.id;
    let direct = SeaOrmHostRepository::new(ctx.db.clone())
        .ensure_direct_space("Direct messages")
        .await
        .unwrap();
    for (server, user, role) in [
        (acme, ada.id, ServerMemberRole::Owner),
        (acme, bob.id, ServerMemberRole::Moderator),
        (beta, ada.id, ServerMemberRole::Owner),
        (beta, carol.id, ServerMemberRole::Member),
        (direct, ada.id, ServerMemberRole::Guest),
        (direct, bob.id, ServerMemberRole::Guest),
        (direct, carol.id, ServerMemberRole::Guest),
    ] {
        members.add_member(server, user, role).await.unwrap();
    }
    let repo = SeaOrmDmPolicyRepository::new(ctx.db.clone());
    People {
        ctx,
        repo,
        ada,
        acme,
        beta,
        direct,
    }
}

#[tokio::test]
async fn a_workspace_starts_with_no_floor_and_direct_members_and_keeps_what_its_owner_sets() {
    let p = people().await;
    let policy = p.repo.workspace_policy(p.acme).await.unwrap().unwrap();
    assert_eq!(policy.outside_floor, OutsideFloor::None);
    assert_eq!(policy.member_default, MemberDm::Direct);

    let set = p
        .repo
        .set_workspace_policy(p.acme, OutsideFloor::Closed, MemberDm::Requests)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(set.server_id, p.acme);
    assert_eq!(set.outside_floor, OutsideFloor::Closed);
    assert_eq!(set.member_default, MemberDm::Requests);
    assert_eq!(p.repo.workspace_policy(p.acme).await.unwrap(), Some(set));
    let beta = p.repo.workspace_policy(p.beta).await.unwrap().unwrap();
    assert_eq!(
        beta.outside_floor,
        OutsideFloor::None,
        "other workspaces keep theirs"
    );

    // The direct space and an unknown id have no rules.
    for id in [p.direct, ServerId::new()] {
        assert_eq!(p.repo.workspace_policy(id).await.unwrap(), None);
        assert_eq!(
            p.repo
                .set_workspace_policy(id, OutsideFloor::Requests, MemberDm::Direct)
                .await
                .unwrap(),
            None
        );
    }
}

#[tokio::test]
async fn a_users_outside_choice_is_requests_until_they_choose() {
    let p = people().await;
    assert_eq!(p.repo.outside(p.ada.id).await.unwrap(), OutsideDm::Requests);
    p.repo.set_outside(p.ada.id, OutsideDm::None).await.unwrap();
    assert_eq!(p.repo.outside(p.ada.id).await.unwrap(), OutsideDm::None);
    p.repo
        .set_outside(p.ada.id, OutsideDm::Requests)
        .await
        .unwrap();
    assert_eq!(p.repo.outside(p.ada.id).await.unwrap(), OutsideDm::Requests);
}

#[tokio::test]
async fn a_users_workspaces_carry_their_rules_and_the_users_choice() {
    let p = people().await;
    p.repo
        .set_workspace_policy(p.beta, OutsideFloor::Requests, MemberDm::Requests)
        .await
        .unwrap();
    p.repo
        .set_member_choice(p.ada.id, p.acme, MemberDm::Requests)
        .await
        .unwrap();
    p.repo
        .set_member_choice(p.ada.id, p.acme, MemberDm::Direct)
        .await
        .unwrap();

    let workspaces = p.repo.user_workspaces(p.ada.id).await.unwrap();
    let summary: Vec<_> = workspaces
        .iter()
        .map(|w| (w.name.as_str(), w.outside_floor, w.member_default, w.choice))
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "Acme",
                OutsideFloor::None,
                MemberDm::Direct,
                Some(MemberDm::Direct)
            ),
            ("Beta", OutsideFloor::Requests, MemberDm::Requests, None),
        ],
        "the direct space is no workspace; a choice set twice keeps the last"
    );
    assert_eq!(workspaces[1].members(), MemberDm::Requests);
}

#[tokio::test]
async fn shared_workspaces_name_the_senders_role_and_leave_out_the_direct_space() {
    let p = people().await;
    p.repo
        .set_member_choice(p.ada.id, p.acme, MemberDm::Requests)
        .await
        .unwrap();

    let with_bob = p
        .repo
        .shared_workspaces(p.ada.id, "rimp1_bob")
        .await
        .unwrap();
    assert_eq!(with_bob.len(), 1);
    assert_eq!(with_bob[0].workspace.server_id, p.acme);
    assert_eq!(with_bob[0].sender_role, ServerMemberRole::Moderator);
    assert_eq!(with_bob[0].workspace.choice, Some(MemberDm::Requests));

    let with_carol = p
        .repo
        .shared_workspaces(p.ada.id, "rimp1_carol")
        .await
        .unwrap();
    assert_eq!(with_carol.len(), 1);
    assert_eq!(with_carol[0].workspace.server_id, p.beta);
    assert_eq!(with_carol[0].sender_role, ServerMemberRole::Member);

    let dave = p.ctx.create_key_only_user("dave", "rimp1_dave").await;
    SeaOrmServerMemberRepository::new(p.ctx.db.clone())
        .add_member(p.direct, dave.id, ServerMemberRole::Guest)
        .await
        .unwrap();
    assert!(
        p.repo
            .shared_workspaces(dave.id, "rimp1_ada")
            .await
            .unwrap()
            .is_empty(),
        "sharing only the direct space shares no workspace"
    );
    assert!(
        p.repo
            .shared_workspaces(p.ada.id, "rimp1_ada")
            .await
            .unwrap()
            .is_empty(),
        "nobody shares a workspace with themselves"
    );
}

#[tokio::test]
async fn the_migrations_go_back_and_forth_with_settings_stored() {
    let p = people().await;
    p.repo.set_outside(p.ada.id, OutsideDm::None).await.unwrap();
    p.repo
        .set_member_choice(p.ada.id, p.acme, MemberDm::Requests)
        .await
        .unwrap();
    let conn = support::db_conn().await;
    let names: Vec<String> = Migrator::migrations()
        .iter()
        .map(|m| m.name().to_string())
        .collect();
    let index = names
        .iter()
        .position(|n| n == FIRST_MIGRATION)
        .expect("migration must be registered");
    let steps = u32::try_from(names.len() - index).expect("steps fit u32");
    let columns = "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
         WHERE table_name = 'server' AND column_name = 'dm_outside_floor') AS present";
    let table = "SELECT to_regclass('public.user_workspace_dm_setting') IS NOT NULL AS present";

    Migrator::down(&conn, Some(steps)).await.expect("down");
    assert!(!exists(&conn, columns).await);
    assert!(!exists(&conn, table).await);
    Migrator::up(&conn, None).await.expect("up");
    assert!(exists(&conn, columns).await);
    assert!(exists(&conn, table).await);
}
