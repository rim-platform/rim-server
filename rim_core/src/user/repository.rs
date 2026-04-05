use crate::repository::error::RepoResult;
use crate::user::model::{NewUser, User, UserId};
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

    async fn any_users_exist(&self) -> RepoResult<bool>;

    async fn update_profile(
        &self,
        id: UserId,
        display_name: Option<Option<String>>,
        avatar_url: Option<Option<String>>,
        status_text: Option<Option<String>>,
        bio: Option<Option<String>>,
    ) -> RepoResult<User>;

    async fn update_password(&self, id: UserId, password_hash: String) -> RepoResult<()>;
}
