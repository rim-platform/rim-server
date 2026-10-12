//! Instance-wide caps an operator sets with `RIM_LIMIT_*`, and the refusal a service answers
//! when one is reached.

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
}
