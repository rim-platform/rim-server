//! A local account's password, checked with OPAQUE: its registration record, its server share,
//! the wrong tries counted against it, and the short-lived rows between the steps of a login,
//! a change and a reset.

use chrono::{DateTime, Duration, Utc};

use crate::model_id;
use crate::user::model::UserId;

model_id!(PasswordLoginId);
model_id!(PasswordRewrapId);
model_id!(PasswordResetId);

/// A local account's password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPassword {
    pub user_id: UserId,
    /// The OPAQUE registration record.
    pub registration: Vec<u8>,
    /// The account's server share, sealed under the server's password key.
    pub share_sealed: Vec<u8>,
    /// Login starts since the last successful login.
    pub failed_attempts: u32,
    /// The next login start is refused before this.
    pub next_attempt_at: Option<DateTime<Utc>>,
    /// When the password locked; only a reset with the recovery key opens it.
    pub locked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// How many tries are free, how long each later try makes the next one wait, and at which try
/// the password locks. A try is a login start; a successful finish clears the count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptSchedule {
    /// The wait after the n-th try, in seconds, at index n - 1; 0 for none. Tries past the end
    /// wait as long as the last entry says.
    pub waits_seconds: Vec<i64>,
    /// The try that locks the password.
    pub lock_after: u32,
}

impl Default for AttemptSchedule {
    /// Tries 1 to 3 are free; after the 4th the next waits 1 minute, after the 5th 5 minutes,
    /// the 6th 15 minutes, the 7th 1 hour, the 8th 4 hours, the 9th 12 hours; the 10th locks.
    fn default() -> Self {
        Self {
            waits_seconds: vec![0, 0, 0, 60, 300, 900, 3600, 14_400, 43_200],
            lock_after: 10,
        }
    }
}

impl AttemptSchedule {
    /// The wait the `count`-th try imposes on the next one; `None` when there is none.
    pub fn wait_after(&self, count: u32) -> Option<Duration> {
        if count == 0 {
            return None;
        }
        let index = usize::try_from(count - 1).unwrap_or(usize::MAX);
        let seconds = self
            .waits_seconds
            .get(index)
            .or(self.waits_seconds.last())
            .copied()
            .unwrap_or(0);
        (seconds > 0).then(|| Duration::seconds(seconds))
    }

    /// Whether the `count`-th try locks the password.
    pub fn locks_at(&self, count: u32) -> bool {
        count >= self.lock_after
    }
}

/// What counting a login start against an account's password found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptStart {
    /// The try is counted; `registration` is the record to answer it with.
    Counted {
        registration: Vec<u8>,
        failed_attempts: u32,
    },
    /// The password is locked; nothing was counted.
    Locked,
    /// Too soon after the last try; nothing was counted.
    Wait { until: DateTime<Utc> },
    /// The account has no password.
    NoPassword,
}

/// What an OPAQUE login state is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordLoginPurpose {
    /// Signing in.
    Login,
    /// Proving the current password before a change.
    Change,
}

impl PasswordLoginPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Change => "change",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "login" => Some(Self::Login),
            "change" => Some(Self::Change),
            _ => None,
        }
    }
}

/// The server's OPAQUE login state between a start and its finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordLogin {
    pub id: PasswordLoginId,
    pub user_id: Option<UserId>,
    pub purpose: PasswordLoginPurpose,
    pub state: Vec<u8>,
    pub expires_at: DateTime<Utc>,
}

/// A password reset between its start and its finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordReset {
    pub id: PasswordResetId,
    pub user_id: UserId,
    pub expires_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_schedule_frees_three_tries_then_waits_longer_and_locks_at_ten() {
        let schedule = AttemptSchedule::default();
        let waits: Vec<Option<i64>> = (1..=9)
            .map(|n| schedule.wait_after(n).map(|d| d.num_seconds()))
            .collect();
        assert_eq!(
            waits,
            vec![
                None,
                None,
                None,
                Some(60),
                Some(300),
                Some(900),
                Some(3600),
                Some(14_400),
                Some(43_200)
            ]
        );
        assert!(!schedule.locks_at(9));
        assert!(schedule.locks_at(10));
        assert_eq!(schedule.wait_after(0), None);
    }

    #[test]
    fn purposes_round_trip() {
        for purpose in [PasswordLoginPurpose::Login, PasswordLoginPurpose::Change] {
            assert_eq!(PasswordLoginPurpose::parse(purpose.as_str()), Some(purpose));
        }
        assert_eq!(PasswordLoginPurpose::parse("reset"), None);
    }
}
