use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};

use crate::conversation::model::ConversationId;
use crate::message::model::MessageId;
use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;

model_id!(MessageReportId);

/// A user banned from a workspace (Track 1.2). While the ban is active (no `expires_at`, or
/// `expires_at` in the future) every workspace join path refuses the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerBan {
    pub server_id: ServerId,
    pub user_id: UserId,
    pub reason: Option<String>,
    /// `None` once the banning moderator's account is deleted.
    pub banned_by: Option<UserId>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl ServerBan {
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_none_or(|expires_at| expires_at > now)
    }
}

/// A member muted until `until` (Track 1.2). Only a timeout with `until` in the future takes
/// effect; expired rows are ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberTimeout {
    pub server_id: ServerId,
    pub user_id: UserId,
    pub until: DateTime<Utc>,
    pub reason: Option<String>,
    /// `None` once the moderator's account is deleted.
    pub set_by: Option<UserId>,
    pub created_at: DateTime<Utc>,
}

/// Lifecycle of a [`MessageReport`]. Persisted as the snake_case string of [`Self::as_str`] in
/// the Postgres enum `message_report_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageReportStatus {
    Open,
    Resolved,
    Dismissed,
}

impl MessageReportStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageReportStatus::Open => "open",
            MessageReportStatus::Resolved => "resolved",
            MessageReportStatus::Dismissed => "dismissed",
        }
    }
}

impl fmt::Display for MessageReportStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseMessageReportStatusError {
    input: String,
}

impl fmt::Display for ParseMessageReportStatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown message report status: {}", self.input)
    }
}

impl std::error::Error for ParseMessageReportStatusError {}

impl FromStr for MessageReportStatus {
    type Err = ParseMessageReportStatusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "open" => Ok(MessageReportStatus::Open),
            "resolved" => Ok(MessageReportStatus::Resolved),
            "dismissed" => Ok(MessageReportStatus::Dismissed),
            other => Err(ParseMessageReportStatusError {
                input: other.to_string(),
            }),
        }
    }
}

/// A member's report of a message to the workspace moderators (Track 1.2). One per
/// `(message_id, reporter_user_id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageReport {
    pub id: MessageReportId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub reporter_user_id: UserId,
    pub reason: String,
    pub status: MessageReportStatus,
    pub resolved_by: Option<UserId>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessageReport {
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    pub reporter_user_id: UserId,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_status_round_trips() {
        for status in [
            MessageReportStatus::Open,
            MessageReportStatus::Resolved,
            MessageReportStatus::Dismissed,
        ] {
            assert_eq!(status.as_str().parse::<MessageReportStatus>(), Ok(status));
        }
        assert!("closed".parse::<MessageReportStatus>().is_err());
    }

    #[test]
    fn ban_activity_respects_expiry() {
        let now = Utc::now();
        let mut ban = ServerBan {
            server_id: ServerId::new(),
            user_id: UserId::new(),
            reason: None,
            banned_by: None,
            created_at: now,
            expires_at: None,
        };
        assert!(ban.is_active_at(now));
        ban.expires_at = Some(now + chrono::Duration::minutes(1));
        assert!(ban.is_active_at(now));
        ban.expires_at = Some(now - chrono::Duration::minutes(1));
        assert!(!ban.is_active_at(now));
    }
}
