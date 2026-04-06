use async_trait::async_trait;

use rim_services::user::command::{GetMeQuery, GetUserQuery, UpdateMyProfileCmd};
use rim_services::user::dto::UserDto;
use rim_services::user::service::UserService;
use rim_services::{ServiceError, ServiceResult};

use crate::UserRepos;

/// Strip control characters (except newline/tab in bio) from a string.
fn strip_control_chars(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

/// Strip control characters for single-line fields (no newlines/tabs).
fn strip_control_chars_single_line(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

#[derive(Clone)]
pub struct DefaultUserService {
    repos: UserRepos,
}

impl DefaultUserService {
    pub fn new(repos: UserRepos) -> Self {
        Self { repos }
    }

    fn convert_to_dto(user: rim_core::user::model::User) -> UserDto {
        UserDto {
            id: user.id,
            username: user.username,
            display_name: user.display_name,
            avatar_url: user.avatar_url,
            status_text: user.status_text,
            bio: user.bio,
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

    async fn get_users_by_ids(
        &self,
        ids: &[rim_core::user::model::UserId],
    ) -> ServiceResult<Vec<UserDto>> {
        let users = self
            .repos
            .user_repo
            .get_by_ids(ids)
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(users.into_iter().map(Self::convert_to_dto).collect())
    }

    async fn update_my_profile(&self, cmd: UpdateMyProfileCmd) -> ServiceResult<UserDto> {
        // Sanitize and validate display_name.
        let display_name = cmd.display_name.map(|v| {
            let v = strip_control_chars_single_line(v.trim());
            if v.is_empty() { None } else { Some(v) }
        });
        if let Some(Some(ref dn)) = display_name {
            if dn.chars().count() > 64 {
                return Err(ServiceError::Validation(
                    "display_name must be at most 64 characters".to_string(),
                ));
            }
        }

        // Sanitize and validate status_text.
        let status_text = cmd.status_text.map(|v| {
            let v = strip_control_chars_single_line(v.trim());
            if v.is_empty() { None } else { Some(v) }
        });
        if let Some(Some(ref st)) = status_text {
            if st.chars().count() > 256 {
                return Err(ServiceError::Validation(
                    "status_text must be at most 256 characters".to_string(),
                ));
            }
        }

        // Sanitize and validate bio.
        let bio = cmd.bio.map(|v| {
            let v = strip_control_chars(v.trim());
            if v.is_empty() { None } else { Some(v) }
        });
        if let Some(Some(ref b)) = bio {
            if b.chars().count() > 2000 {
                return Err(ServiceError::Validation(
                    "bio must be at most 2000 characters".to_string(),
                ));
            }
        }

        // Validate avatar_url: must be internal path.
        let avatar_url = cmd.avatar_url.map(|v| {
            let v = v.trim().to_string();
            if v.is_empty() { None } else { Some(v) }
        });
        if let Some(Some(ref url)) = avatar_url {
            if !url.starts_with("/api/files/") {
                return Err(ServiceError::Validation(
                    "avatar_url must start with /api/files/".to_string(),
                ));
            }
        }

        let user = self
            .repos
            .user_repo
            .update_profile(
                cmd.actor.user_id,
                display_name,
                avatar_url,
                status_text,
                bio,
            )
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(Self::convert_to_dto(user))
    }
}
