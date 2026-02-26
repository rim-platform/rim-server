use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::user::command::{GetMeQuery, GetUserQuery, UpdateMyProfileCmd};
use crate::user::dto::UserDto;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait UserService: Send + Sync {
    async fn get_me(&self, query: GetMeQuery) -> ServiceResult<UserDto>;

    async fn get_user(&self, query: GetUserQuery) -> ServiceResult<UserDto>;

    async fn update_my_profile(&self, cmd: UpdateMyProfileCmd) -> ServiceResult<UserDto>;
}
