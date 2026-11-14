//! A local account's password, checked with OPAQUE: its registration record, its server share,
//! the login starts counted against it, and the short-lived rows between the steps of a login,
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
    /// Login starts counted in the current window since the last successful login.
    pub failed_attempts: u32,
    /// When the current window of counted starts ends; `None` before the first start.
    pub window_ends_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// How many login starts an account's password takes in one window, and how long a window
/// lasts. The first start after a window ended opens the next one; a start past the limit waits
/// until the window ends. A successful finish clears the count. Nothing locks the password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptWindow {
    /// The starts one window takes.
    pub max_starts: u32,
    /// How long a window lasts.
    pub length: Duration,
}

impl Default for AttemptWindow {
    /// Ten starts an hour.
    fn default() -> Self {
        Self {
            max_starts: 10,
            length: Duration::hours(1),
        }
    }
}

impl AttemptWindow {
    /// Whether a window ending at `ends_at` is still open at `now`. An end further than one
    /// window length after `now` counts as ended, so no stored time holds a start back longer
    /// than one window.
    pub fn is_open(&self, ends_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
        ends_at.is_some_and(|end| end > now && end <= now + self.length)
    }

    /// Count one more start at `now` against `count` starts in a window ending at `ends_at`.
    /// Returns the new count and window end, or, when the window is full, when it ends.
    ///
    /// # Errors
    /// The end of the open window when it already took [`Self::max_starts`] starts.
    pub fn count(
        &self,
        count: u32,
        ends_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<(u32, DateTime<Utc>), DateTime<Utc>> {
        match ends_at {
            Some(end) if self.is_open(ends_at, now) => {
                if count >= self.max_starts {
                    Err(end)
                } else {
                    Ok((count + 1, end))
                }
            }
            _ => Ok((1, now + self.length)),
        }
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
    /// The window is full until `until`; nothing was counted.
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
    fn a_window_takes_ten_starts_an_hour_and_the_next_opens_once_it_ends() {
        let window = AttemptWindow::default();
        let now = Utc::now();
        let end = now + Duration::hours(1);
        assert_eq!(window.count(0, None, now), Ok((1, end)));
        assert_eq!(window.count(9, Some(end), now), Ok((10, end)));
        assert_eq!(window.count(10, Some(end), now), Err(end));
        assert_eq!(
            window.count(10, Some(end), end - Duration::seconds(1)),
            Err(end)
        );
        assert_eq!(
            window.count(10, Some(end), end),
            Ok((1, end + Duration::hours(1)))
        );
        assert_eq!(
            window
                .count(500, Some(end), end + Duration::days(30))
                .map(|c| c.0),
            Ok(1)
        );
    }

    #[test]
    fn an_end_further_than_one_window_away_counts_as_ended() {
        let window = AttemptWindow::default();
        let now = Utc::now();
        let far = now + Duration::hours(12);
        assert!(!window.is_open(Some(far), now));
        assert_eq!(
            window.count(9, Some(far), now),
            Ok((1, now + Duration::hours(1)))
        );
    }

    #[test]
    fn purposes_round_trip() {
        for purpose in [PasswordLoginPurpose::Login, PasswordLoginPurpose::Change] {
            assert_eq!(PasswordLoginPurpose::parse(purpose.as_str()), Some(purpose));
        }
        assert_eq!(PasswordLoginPurpose::parse("reset"), None);
    }
}
