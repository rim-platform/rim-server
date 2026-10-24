//! Live-database tests for workspace invite redemption: an invite revoked or expired after the
//! join's pre-check is refused by the consuming update itself.

#![cfg(feature = "infra-db-tests")]

mod support;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

use rim_auth::session_token::hash_token;
use rim_core::auth::model::{Invite, InviteId, NewInvite};
use rim_core::auth::repository::{InviteCursor, InviteRepository, ListInvitesParams};
use rim_core::repository::error::RepoResult;
use rim_core::repository::pagination::Page;
use rim_core::server::model::ServerId;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::{NewUser, NewUserKeyIdentity, UserId};
use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::invite::SeaOrmInviteRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_infra_services::{AppRepos, DefaultAppServiceProvider};
use rim_services::auth::command::{JoinServerByInviteCmd, SignupToken};
use rim_services::provider::AppServiceProvider;
use rim_services::{Actor, ServiceError};

/// The stored invite as the pre-check read it; the row is then changed by `after_read`, as a
/// concurrent revoke or the clock passing the expiry would change it before the consume.
struct ChangedAfterRead {
    inner: SeaOrmInviteRepository,
    db: sea_orm::DatabaseConnection,
    after_read: &'static str,
}

#[async_trait]
impl InviteRepository for ChangedAfterRead {
    async fn get_by_id(&self, id: InviteId) -> RepoResult<Option<Invite>> {
        self.inner.get_by_id(id).await
    }

    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Invite>> {
        let invite = self.inner.get_by_token_hash(token_hash).await?;
        if let Some(invite) = &invite {
            self.db
                .execute_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    self.after_read,
                    [sea_orm::prelude::Uuid::from(invite.id).into()],
                ))
                .await
                .expect("change the invite after the read");
        }
        Ok(invite)
    }

    async fn create_invite(&self, invite: NewInvite) -> RepoResult<Invite> {
        self.inner.create_invite(invite).await
    }

    async fn try_consume(&self, id: InviteId) -> RepoResult<bool> {
        self.inner.try_consume(id).await
    }

    async fn revoke(&self, id: InviteId, at: chrono::DateTime<Utc>) -> RepoResult<()> {
        self.inner.revoke(id, at).await
    }

    async fn list_for_server(
        &self,
        params: ListInvitesParams,
    ) -> RepoResult<Page<Invite, InviteCursor>> {
        self.inner.list_for_server(params).await
    }

    async fn count_created_since(
        &self,
        user_id: UserId,
        since: chrono::DateTime<Utc>,
    ) -> RepoResult<u64> {
        self.inner.count_created_since(user_id, since).await
    }
}

async fn user(handle: &DbHandle, name: &str) -> UserId {
    SeaOrmUserRepository::new(handle.clone())
        .create_user(NewUser {
            username: name.to_string(),
            email: Some(format!("{name}@example.com")),
            display_name: None,
            key_identity: Some(NewUserKeyIdentity {
                principal_id: format!("rimp1_{name}"),
                identity_key: format!("ed25519:{name}"),
            }),
        })
        .await
        .expect("create user")
        .id
}

/// The raw token of the invite every test redeems.
const RAW_TOKEN: &str = "rim_inv_changed_after_read";

/// A workspace with one invite, and services whose invite pre-check is followed by
/// `after_read` changing the invite's row.
struct Race {
    provider: DefaultAppServiceProvider,
    handle: DbHandle,
    server_id: ServerId,
}

impl Race {
    async fn new(after_read: &'static str) -> Self {
        let db = support::db_conn().await;
        support::reset_db(&db).await;
        let handle = DbHandle::Connection(db.clone());

        let owner = user(&handle, "inviteowner").await;
        let server = SeaOrmServerRepository::new(handle.clone())
            .create_server("invite-race")
            .await
            .expect("create server");
        SeaOrmServerMemberRepository::new(handle.clone())
            .add_member(server.id, owner, ServerMemberRole::Owner)
            .await
            .expect("add owner");
        SeaOrmInviteRepository::new(handle.clone())
            .create_invite(NewInvite {
                server_id: server.id,
                created_by_user_id: owner,
                token_hash: hash_token(RAW_TOKEN),
                expires_at: Some(Utc::now() + Duration::days(1)),
                max_uses: 5,
                role: ServerMemberRole::Member,
            })
            .await
            .expect("create invite");

        let mut repos = AppRepos::with_handle(&handle);
        repos.invite_repo = Arc::new(ChangedAfterRead {
            inner: SeaOrmInviteRepository::new(handle.clone()),
            db: db.clone(),
            after_read,
        });
        let provider = support::provider(db, &repos, Default::default());
        Self {
            provider,
            handle,
            server_id: server.id,
        }
    }

    /// An existing user joins with the invite; returns the result and whether they became a
    /// member.
    async fn join(&self) -> (Result<(), ServiceError>, bool) {
        let joiner = user(&self.handle, "joiner").await;
        let result = self
            .provider
            .auth_service()
            .join_server_by_invite(JoinServerByInviteCmd {
                actor: Actor::new(joiner),
                invite_token: RAW_TOKEN.to_string(),
            })
            .await
            .map(|_| ());
        let member = SeaOrmServerMemberRepository::new(self.handle.clone())
            .is_member(self.server_id, joiner)
            .await
            .expect("membership");
        (result, member)
    }

    /// A newcomer signs up with a key and the invite; returns the result and whether the
    /// account exists.
    async fn sign_up(&self) -> (Result<(), ServiceError>, bool) {
        let result = support::key_signup(
            &self.provider,
            "newcomer",
            7,
            Some(SignupToken::WorkspaceInvite(RAW_TOKEN.to_string())),
            "203.0.113.9",
        )
        .await
        .map(|_| ());
        let account = SeaOrmUserRepository::new(self.handle.clone())
            .find_by_username("newcomer")
            .await
            .expect("user lookup")
            .is_some();
        (result, account)
    }
}

const REVOKE: &str = "UPDATE invite SET revoked_at = now() WHERE id = $1";
const EXPIRE: &str = "UPDATE invite SET expires_at = now() - interval '1 second' WHERE id = $1";

/// An invite revoked after the join's pre-check admits nobody.
#[tokio::test]
async fn a_join_with_an_invite_revoked_after_the_pre_check_is_refused() {
    let (result, member) = Race::new(REVOKE).await.join().await;
    assert!(
        matches!(result, Err(ServiceError::PermissionDenied)),
        "got {result:?}"
    );
    assert!(!member, "a revoked invite must not admit the joiner");
}

/// An invite that expires after the join's pre-check admits nobody.
#[tokio::test]
async fn a_join_with_an_invite_expired_after_the_pre_check_is_refused() {
    let (result, member) = Race::new(EXPIRE).await.join().await;
    assert!(
        matches!(result, Err(ServiceError::PermissionDenied)),
        "got {result:?}"
    );
    assert!(!member, "an expired invite must not admit the joiner");
}

/// A signup with an invite revoked after the pre-check reads as an unknown invite and creates
/// no account.
#[tokio::test]
async fn a_signup_with_an_invite_revoked_after_the_pre_check_is_refused() {
    let (result, account) = Race::new(REVOKE).await.sign_up().await;
    assert!(
        matches!(result, Err(ServiceError::InvalidToken)),
        "got {result:?}"
    );
    assert!(!account, "the refused signup must roll back");
}

/// A signup with an invite whose stored expiry moved into the past after the pre-check is
/// refused and creates no account. The pre-check's copy still reads unexpired, so the refusal
/// reads as an unknown invite.
#[tokio::test]
async fn a_signup_with_an_invite_expired_after_the_pre_check_is_refused() {
    let (result, account) = Race::new(EXPIRE).await.sign_up().await;
    assert!(
        matches!(result, Err(ServiceError::InvalidToken)),
        "got {result:?}"
    );
    assert!(!account, "the refused signup must roll back");
}
