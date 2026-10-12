use async_graphql::{InputObject, SimpleObject};

/// Ask for the signed-in account to be deleted.
#[derive(InputObject)]
pub struct RequestAccountDeletionInput {
    /// The account's username, typed again to confirm.
    pub confirm_username: String,
}

/// A pending account deletion.
#[derive(SimpleObject, Clone, Copy, Debug)]
pub struct AccountDeletionSchedule {
    /// When the grace period ends and the account is purged. Until then the account works as
    /// before and `cancelAccountDeletion` keeps it.
    pub scheduled_for: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::account_deletion::dto::AccountDeletionScheduleDto>
    for AccountDeletionSchedule
{
    fn from(value: rim_services::account_deletion::dto::AccountDeletionScheduleDto) -> Self {
        Self {
            scheduled_for: value.scheduled_for,
        }
    }
}
