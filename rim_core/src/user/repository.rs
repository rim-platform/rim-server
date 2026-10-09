use crate::repository::error::RepoResult;
use crate::user::model::{NewUser, UpdateProfilePatch, User, UserId};
use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn get_by_id(&self, id: UserId) -> RepoResult<Option<User>>;

    async fn get_by_ids(&self, ids: &[UserId]) -> RepoResult<Vec<User>>;

    async fn create_user(&self, user_data: NewUser) -> RepoResult<User>;
    async fn find_by_username(&self, username: &str) -> RepoResult<Option<User>>;

    async fn delete_by_username(&self, username: &str) -> RepoResult<()>;

    async fn update_profile(&self, id: UserId, patch: UpdateProfilePatch) -> RepoResult<User>;

    async fn update_password(&self, id: UserId, password_hash: String) -> RepoResult<()>;

    /// Find a user by email address using a case-insensitive comparison.
    ///
    /// The lookup uses `LOWER(email) = LOWER($1)` so that `"Foo@Bar.COM"` and
    /// `"foo@bar.com"` resolve to the same row. Returns `None` when no row
    /// matches.
    ///
    /// # Errors
    /// Propagates any underlying storage error as [`RepoError`].
    async fn find_by_email(&self, email: &str) -> RepoResult<Option<User>>;
}
