use async_trait::async_trait;

use rim_services::user::command::{GetMeQuery, GetUserQuery, UpdateMyProfileCmd};
use rim_services::user::dto::UserDto;
use rim_services::user::service::UserService;
use rim_services::{ServiceError, ServiceResult};

use crate::UserRepos;

#[cfg(test)]
use crate::test_support::user::{UserRepoPortAdapter, UserReposPorts};

#[derive(Clone)]
pub struct DefaultUserService {
    repos: UserRepos,
}

impl DefaultUserService {
    pub fn new(repos: UserRepos) -> Self {
        Self { repos }
    }

    #[cfg(test)]
    pub fn new_for_tests(repos: UserReposPorts) -> Self {
        Self::new(UserRepos {
            user_repo: std::sync::Arc::new(UserRepoPortAdapter(repos.user_repo)),
        })
    }

    fn convert_to_dto(user: rim_core::user::model::User) -> UserDto {
        UserDto {
            id: user.id,
            username: user.username,
            display_name: None,
            avatar_url: None,
            status_text: None,
        }
    }
}

#[async_trait]
impl UserService for DefaultUserService {
    async fn get_me(&self, query: GetMeQuery) -> ServiceResult<UserDto> {
        let user = self
            .repos
            .user_repo
            .get_by_id(query.actor.user_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        Ok(Self::convert_to_dto(user))
    }

    async fn get_user(&self, query: GetUserQuery) -> ServiceResult<UserDto> {
        let user = self
            .repos
            .user_repo
            .get_by_id(query.user_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        Ok(Self::convert_to_dto(user))
    }

    async fn update_my_profile(&self, _cmd: UpdateMyProfileCmd) -> ServiceResult<UserDto> {
        Err(ServiceError::Validation(
            "profile updates are not supported yet".to_string(),
        ))
    }
}
