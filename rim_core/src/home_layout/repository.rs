use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::home_layout::model::HomeLayout;
use crate::host::model::UserKind;
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

/// Stored Home layouts, one per account.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait HomeLayoutRepository: Send + Sync {
    /// What the `user` row of `user_id` is, or `None` when there is no such row.
    async fn account_kind(&self, user_id: UserId) -> RepoResult<Option<UserKind>>;

    /// The account's stored layout, or `None` when none is stored.
    async fn get(&self, user_id: UserId) -> RepoResult<Option<HomeLayout>>;

    /// Store `layout` with `edited_at` unless the stored layout's `edited_at` is later, in one
    /// statement. Returns the stored row when the new layout was written, `None` when the stored
    /// one was kept.
    async fn save_unless_newer(
        &self,
        user_id: UserId,
        layout: serde_json::Value,
        edited_at: DateTime<Utc>,
    ) -> RepoResult<Option<HomeLayout>>;

    /// Delete the account's layout. Returns whether one was stored.
    async fn delete(&self, user_id: UserId) -> RepoResult<bool>;
}
