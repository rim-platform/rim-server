/// Seed helpers for registration-related test data.
///
/// Each function accepts the *raw* token string, hashes it internally using
/// [`rim_auth::session_token::hash_token`], and writes the resulting row to the
/// database.  Test code passes the raw token around and submits it in GraphQL
/// requests; the helper handles the hash so tests read like the real call site.
use chrono::{DateTime, Utc};
use rim_auth::session_token::hash_token;
use rim_core::auth::model::{
    BootstrapTokenId, InstanceInviteId, InviteId, NewBootstrapToken, NewInstanceInvite, NewInvite,
};
use rim_core::auth::repository::{
    BootstrapTokenRepository, InstanceInviteRepository, InviteRepository,
};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::bootstrap_token::SeaOrmBootstrapTokenRepository;
use rim_infra_db::sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository;
use rim_infra_db::sea_orm_impl::invite::SeaOrmInviteRepository;

/// Options for [`seed_instance_invite`].
#[allow(dead_code)]
#[derive(Default)]
pub struct SeedInstanceInviteOpts {
    /// Expiry for the invite row.  Defaults to `now() + 7 days` when `None`.
    pub expires_at: Option<DateTime<Utc>>,
    /// Maximum number of uses.  Defaults to `1` when `None`.
    pub max_uses: Option<i32>,
}

/// Insert a `bootstrap_token` row for the given raw token and return its id.
///
/// Hashes `raw_token` with SHA-256 before inserting.  The caller must supply
/// `raw_token` when submitting the `signupWithInvite` GraphQL mutation.
///
/// # Panics
/// Panics if the database insert fails.
pub async fn seed_bootstrap_token(
    db: &DbHandle,
    raw_token: &str,
    expires_at: DateTime<Utc>,
) -> BootstrapTokenId {
    let repo = SeaOrmBootstrapTokenRepository::new(db.clone());
    let token = repo
        .create(NewBootstrapToken {
            email: "test@example.com".to_string(),
            token_hash: hash_token(raw_token),
            expires_at,
        })
        .await
        .expect("seed_bootstrap_token: insert failed");
    token.id
}

/// Insert an `instance_invite` row for the given raw token and return its id.
///
/// Hashes `raw_token` with SHA-256 before inserting.
///
/// # Panics
/// Panics if the database insert fails.
#[allow(dead_code)]
pub async fn seed_instance_invite(
    db: &DbHandle,
    created_by: UserId,
    raw_token: &str,
    opts: SeedInstanceInviteOpts,
) -> InstanceInviteId {
    let repo = SeaOrmInstanceInviteRepository::new(db.clone());
    let expires_at = opts
        .expires_at
        .unwrap_or_else(|| Utc::now() + chrono::Duration::days(7));
    let max_uses = opts.max_uses.unwrap_or(1);
    let invite = repo
        .create(NewInstanceInvite {
            created_by_user_id: created_by,
            token_hash: hash_token(raw_token),
            expires_at,
            max_uses,
        })
        .await
        .expect("seed_instance_invite: insert failed");
    invite.id
}

/// Insert an `invite` (workspace invite) row for the given raw token and
/// return its id.
///
/// Hashes `raw_token` with SHA-256 before inserting.  The invite has
/// `max_uses = 1` and no expiry.
///
/// # Panics
/// Panics if the database insert fails.
pub async fn seed_workspace_invite(
    db: &DbHandle,
    owner: UserId,
    server: ServerId,
    raw_token: &str,
) -> InviteId {
    let repo = SeaOrmInviteRepository::new(db.clone());
    let invite = repo
        .create_invite(NewInvite {
            server_id: server,
            created_by_user_id: owner,
            token_hash: hash_token(raw_token),
            expires_at: None,
            max_uses: 1,
        })
        .await
        .expect("seed_workspace_invite: insert failed");
    invite.id
}
