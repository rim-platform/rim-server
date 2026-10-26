use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::user::model::UserId;

use crate::ServiceResult;
use crate::user::command::{GetMeQuery, UpdateMyProfileCmd};
use crate::user::dto::UserDto;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait UserService: Send + Sync {
    async fn get_me(&self, query: GetMeQuery) -> ServiceResult<UserDto>;

    async fn get_users_by_ids(&self, ids: &[UserId]) -> ServiceResult<Vec<UserDto>>;

    async fn update_my_profile(&self, cmd: UpdateMyProfileCmd) -> ServiceResult<UserDto>;

    /// When the caller's account leaves probation; `None` once it has, when the instance has
    /// no probation, or when the caller is an instance operator.
    async fn probation_ends_at(
        &self,
        query: GetMeQuery,
    ) -> ServiceResult<Option<chrono::DateTime<chrono::Utc>>>;

    /// Whether `user_id` may upload a profile picture now, counting this attempt: refused while
    /// the profile is locked, while the account is in probation, and past the hourly or daily
    /// upload limit. Operators are exempt from probation and the limits, not from a lock.
    ///
    /// # Errors
    /// `NotFound` for an unknown account, `Forbidden` while the profile is locked, `Validation`
    /// during probation and `RateLimited` past an upload limit.
    async fn begin_avatar_upload(&self, user_id: UserId) -> ServiceResult<()>;

    /// Clear `user_id`'s display name, picture, status and bio, delete their avatar files, lock
    /// profile changes until `locked_until` when given, and announce the change. The caller has
    /// checked it may do this (an operator action).
    ///
    /// # Errors
    /// `NotFound` for an unknown account.
    async fn clear_profile(
        &self,
        user_id: UserId,
        locked_until: Option<DateTime<Utc>>,
    ) -> ServiceResult<UserDto>;
}
