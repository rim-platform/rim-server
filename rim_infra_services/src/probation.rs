//! The probation of new accounts, which never holds back an instance operator.

use chrono::{DateTime, Utc};
use rim_core::user::model::User;
use rim_core::user::repository::UserRepository;
use rim_services::ServiceResult;
use rim_services::limits::InstanceLimits;

/// When `user`'s probation ends, while they are in it at `now`; `None` once it has, when the
/// instance has no probation, or when `user` is an instance operator, who is never on
/// probation. The role is read on each call, so an account granted it leaves probation at
/// once.
pub(crate) async fn probation_ends_at(
    limits: &InstanceLimits,
    user_repo: &dyn UserRepository,
    user: &User,
    now: DateTime<Utc>,
) -> ServiceResult<Option<DateTime<Utc>>> {
    let Some(ends_at) = limits.probation_ends_at(user.created_at, now) else {
        return Ok(None);
    };
    if user_repo.is_instance_operator(user.id).await? {
        return Ok(None);
    }
    Ok(Some(ends_at))
}

/// `Ok` unless `user` is in probation at `now`, as [`probation_ends_at`] judges it.
///
/// # Errors
/// `Validation` saying the account can `action` once probation ends, as
/// [`InstanceLimits::ensure_out_of_probation`] words it.
pub(crate) async fn ensure_out_of_probation(
    limits: &InstanceLimits,
    user_repo: &dyn UserRepository,
    user: &User,
    now: DateTime<Utc>,
    action: &str,
) -> ServiceResult<()> {
    if probation_ends_at(limits, user_repo, user, now)
        .await?
        .is_none()
    {
        return Ok(());
    }
    limits.ensure_out_of_probation(user.created_at, now, action)
}
