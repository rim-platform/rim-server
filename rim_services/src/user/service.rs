use async_trait::async_trait;

use crate::ServiceResult;
use crate::user::command::{GetMeQuery, GetUserQuery, UpdateMyProfileCmd};
use crate::user::dto::{UserDto};

#[async_trait]
pub trait UserService: Send + Sync {
    async fn get_me(&self, query: GetMeQuery) -> ServiceResult<UserDto>;

    async fn get_user(&self, query: GetUserQuery) -> ServiceResult<UserDto>;

    async fn update_my_profile(&self, cmd: UpdateMyProfileCmd) -> ServiceResult<UserDto>;
}
