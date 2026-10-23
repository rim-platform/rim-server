//! A workspace's public join link and the join requests filed through it.

use chrono::{DateTime, Utc};

use crate::model_id;
use crate::server::model::ServerId;
use crate::server_member::model::ServerMemberRole;
use crate::user::model::UserId;

model_id!(JoinRequestId);

/// What opening a public link does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PublicLinkMode {
    /// The person joins at once.
    #[default]
    Direct,
    /// The person files a join request that a moderator accepts or declines.
    Requests,
}

impl PublicLinkMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Requests => "requests",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "direct" => Some(Self::Direct),
            "requests" => Some(Self::Requests),
            _ => None,
        }
    }
}

/// A workspace's public link: the token's hash for lookups, the token sealed under the server's
/// link key, and the Owner's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicJoinLink {
    pub server_id: ServerId,
    /// Hex SHA-256 of the token.
    pub token_hash: String,
    /// The token sealed under the server's link key.
    pub sealed_token: Vec<u8>,
    pub enabled: bool,
    pub mode: PublicLinkMode,
    /// The role a joiner gets: `Member` or `Guest`.
    pub role: ServerMemberRole,
    /// Whether accounts from other servers may join through the link.
    pub allow_remote: bool,
    /// When the link was last turned on; `None` while it never was.
    pub enabled_at: Option<DateTime<Utc>>,
    /// When the current token was issued.
    pub issued_at: DateTime<Utc>,
    /// Who issued the current token; `None` once that account is gone.
    pub issued_by: Option<UserId>,
}

/// Where a join request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinRequestStatus {
    Open,
    Accepted,
    Declined,
    /// The person took it back.
    Withdrawn,
    /// A ban or the person's account deletion ended it.
    Cancelled,
    /// It stayed open past its expiry.
    Expired,
}

impl JoinRequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Withdrawn => "withdrawn",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            "accepted" => Some(Self::Accepted),
            "declined" => Some(Self::Declined),
            "withdrawn" => Some(Self::Withdrawn),
            "cancelled" => Some(Self::Cancelled),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

/// A person's request to join a workspace through its public link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinRequest {
    pub id: JoinRequestId,
    pub server_id: ServerId,
    pub user_id: UserId,
    /// The person's optional note to the moderators.
    pub message: Option<String>,
    /// The stored status; an `Open` request past `expires_at` counts as expired (see
    /// [`JoinRequest::is_open_at`]).
    pub status: JoinRequestStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by: Option<UserId>,
}

impl JoinRequest {
    /// Whether the request is still open at `now`: stored as open and not past its expiry.
    pub fn is_open_at(&self, now: DateTime<Utc>) -> bool {
        self.status == JoinRequestStatus::Open && self.expires_at > now
    }

    /// The status as of `now`: [`JoinRequestStatus::Expired`] for an open request past its
    /// expiry, the stored status otherwise.
    pub fn status_at(&self, now: DateTime<Utc>) -> JoinRequestStatus {
        if self.status == JoinRequestStatus::Open && self.expires_at <= now {
            JoinRequestStatus::Expired
        } else {
            self.status
        }
    }
}

/// A join request to file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJoinRequest {
    pub server_id: ServerId,
    pub user_id: UserId,
    pub message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_and_statuses_round_trip_through_their_names() {
        for mode in [PublicLinkMode::Direct, PublicLinkMode::Requests] {
            assert_eq!(PublicLinkMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(PublicLinkMode::parse("open"), None);
        for status in [
            JoinRequestStatus::Open,
            JoinRequestStatus::Accepted,
            JoinRequestStatus::Declined,
            JoinRequestStatus::Withdrawn,
            JoinRequestStatus::Cancelled,
            JoinRequestStatus::Expired,
        ] {
            assert_eq!(JoinRequestStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(JoinRequestStatus::parse("pending"), None);
    }

    #[test]
    fn an_open_request_past_its_expiry_counts_as_expired() {
        let now = Utc::now();
        let request = JoinRequest {
            id: JoinRequestId::new(),
            server_id: ServerId::new(),
            user_id: UserId::new(),
            message: None,
            status: JoinRequestStatus::Open,
            created_at: now,
            expires_at: now + chrono::Duration::days(30),
            decided_at: None,
            decided_by: None,
        };
        assert!(request.is_open_at(now));
        assert_eq!(request.status_at(now), JoinRequestStatus::Open);
        let later = now + chrono::Duration::days(30);
        assert!(!request.is_open_at(later));
        assert_eq!(request.status_at(later), JoinRequestStatus::Expired);
        let declined = JoinRequest {
            status: JoinRequestStatus::Declined,
            ..request
        };
        assert!(!declined.is_open_at(now));
        assert_eq!(declined.status_at(later), JoinRequestStatus::Declined);
    }
}
