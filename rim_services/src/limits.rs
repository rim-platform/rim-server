//! Instance-wide caps an operator sets with `RIM_LIMIT_*`, the probation of new accounts, and
//! the refusals a service answers when one applies.

use chrono::{DateTime, Duration, Utc};

use crate::ServiceError;

/// The instance's caps; a `None` cap is off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InstanceLimits {
    /// Workspaces one user owns.
    pub workspaces_per_user: Option<u32>,
    /// Members of one workspace; the direct space has no cap.
    pub members_per_workspace: Option<u32>,
    /// Channels of one workspace, public and private.
    pub channels_per_workspace: Option<u32>,
    /// Devices on one account: the stricter of `RIM_LIMIT_DEVICES_PER_USER` and the relay
    /// quota.
    pub devices_per_user: Option<u32>,
    /// DM requests one user sends to strangers per day: the stricter of
    /// `RIM_LIMIT_DM_REQUESTS_PER_DAY` and the relay quota.
    pub dm_requests_per_day: Option<u32>,
    /// Macros of one workspace.
    pub macros_per_workspace: Option<u32>,
    /// Pipelines of one workspace.
    pub pipelines_per_workspace: Option<u32>,
    /// Plugins of one workspace's registry.
    pub plugins_per_workspace: Option<u32>,
    /// Invites one user creates per day.
    pub invites_per_user_per_day: Option<u32>,
    /// Uses one invite may have.
    pub invite_max_uses: Option<u32>,
    /// Bytes of one message's text.
    pub message_bytes: Option<u32>,
    /// Hours after its creation during which an account cannot create a workspace or send a
    /// DM request to a stranger, and sends messages at the probation rate.
    pub probation_hours: Option<u32>,
    /// Pending invitations to encrypted conversations one user holds.
    pub pending_invites_per_user: Option<u32>,
}

impl InstanceLimits {
    /// When an account created at `created_at` leaves probation, while `now` is before then;
    /// `None` once it has or when there is no probation.
    pub fn probation_ends_at(
        &self,
        created_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        let ends_at = created_at + Duration::hours(i64::from(self.probation_hours?));
        (now < ends_at).then_some(ends_at)
    }

    /// `Ok` unless an account created at `created_at` is in probation at `now`.
    ///
    /// # Errors
    /// `Validation` saying the account can `action` once probation ends, in minutes rounded
    /// up, e.g. "a new account can create a workspace in 42 minutes".
    pub fn ensure_out_of_probation(
        &self,
        created_at: DateTime<Utc>,
        now: DateTime<Utc>,
        action: &str,
    ) -> Result<(), ServiceError> {
        let Some(ends_at) = self.probation_ends_at(created_at, now) else {
            return Ok(());
        };
        let minutes = ((ends_at - now).num_milliseconds() + 59_999) / 60_000;
        let unit = if minutes == 1 { "minute" } else { "minutes" };
        Err(ServiceError::Validation(format!(
            "a new account can {action} in {minutes} {unit}"
        )))
    }
}

/// The refusal of a write that would pass `cap`, naming the cap and what it counts, e.g.
/// "this instance allows 5 workspaces per user".
pub fn limit_reached(cap: u32, what: &str) -> ServiceError {
    ServiceError::Validation(format!("this instance allows {cap} {what}"))
}

/// `Ok` while `count` is below `cap` or there is no cap, so one more may be added; otherwise
/// [`limit_reached`].
///
/// # Errors
/// `Validation` naming the cap.
pub fn ensure_below(cap: Option<u32>, count: u64, what: &str) -> Result<(), ServiceError> {
    match cap {
        Some(cap) if count >= u64::from(cap) => Err(limit_reached(cap, what)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn below_the_cap_passes_at_it_refuses_naming_it_and_no_cap_never_refuses() {
        assert!(ensure_below(Some(5), 4, "workspaces per user").is_ok());
        assert!(matches!(
            ensure_below(Some(5), 5, "workspaces per user"),
            Err(ServiceError::Validation(m)) if m == "this instance allows 5 workspaces per user"
        ));
        assert!(ensure_below(Some(5), 9, "workspaces per user").is_err());
        assert!(ensure_below(None, u64::MAX, "workspaces per user").is_ok());
    }

    #[test]
    fn probation_lasts_its_hours_from_creation_and_names_the_minutes_left_rounded_up() {
        let limits = InstanceLimits {
            probation_hours: Some(1),
            ..Default::default()
        };
        let created = Utc::now();
        let refusal = |now| limits.ensure_out_of_probation(created, now, "create a workspace");
        assert!(matches!(
            refusal(created),
            Err(ServiceError::Validation(m)) if m == "a new account can create a workspace in 60 minutes"
        ));
        assert!(matches!(
            refusal(created + Duration::minutes(17) + Duration::seconds(1)),
            Err(ServiceError::Validation(m)) if m.ends_with("in 43 minutes")
        ));
        assert!(matches!(
            refusal(created + Duration::seconds(3599)),
            Err(ServiceError::Validation(m)) if m.ends_with("in 1 minute")
        ));
        assert!(refusal(created + Duration::hours(1)).is_ok());
        assert_eq!(
            limits.probation_ends_at(created, created + Duration::minutes(5)),
            Some(created + Duration::hours(1))
        );
        assert_eq!(
            limits.probation_ends_at(created, created + Duration::hours(2)),
            None
        );

        let off = InstanceLimits::default();
        assert_eq!(off.probation_ends_at(created, created), None);
        assert!(
            off.ensure_out_of_probation(created, created, "create a workspace")
                .is_ok()
        );
    }
}
