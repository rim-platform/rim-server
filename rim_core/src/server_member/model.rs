use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};
use std::fmt;
use std::str::FromStr;

model_id!(ServerMemberId);

/// The four-role ladder:
///
/// - `Owner`: everything.
/// - `Moderator`: moderation and channel management; may change roles only between `Member`
///   and `Guest`.
/// - `Member`: the default; reads and writes wherever they are a member, creates channels.
/// - `Guest`: restricted; reads and writes only in conversations they were explicitly added to,
///   cannot create, browse or join channels, invite, or see the full member directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerMemberRole {
    Owner,
    Moderator,
    Member,
    Guest,
}

impl ServerMemberRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServerMemberRole::Owner => "owner",
            ServerMemberRole::Moderator => "moderator",
            ServerMemberRole::Member => "member",
            ServerMemberRole::Guest => "guest",
        }
    }

    /// `Owner` or `Moderator`: may moderate (kick, ban, timeout, delete others' messages, slow
    /// mode, reports) and is exempt from slow mode.
    pub fn is_moderator(&self) -> bool {
        matches!(self, ServerMemberRole::Owner | ServerMemberRole::Moderator)
    }

    pub fn is_guest(&self) -> bool {
        matches!(self, ServerMemberRole::Guest)
    }
}

impl fmt::Display for ServerMemberRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ServerMemberRole {
    type Err = ParseServerMemberRoleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "owner" => Ok(ServerMemberRole::Owner),
            "moderator" => Ok(ServerMemberRole::Moderator),
            "member" => Ok(ServerMemberRole::Member),
            "guest" => Ok(ServerMemberRole::Guest),
            other => Err(ParseServerMemberRoleError::new(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseServerMemberRoleError {
    input: String,
}

impl ParseServerMemberRoleError {
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }
}

impl fmt::Display for ParseServerMemberRoleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown server member role: {}", self.input)
    }
}

impl std::error::Error for ParseServerMemberRoleError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerMember {
    pub id: ServerMemberId,
    pub server_id: ServerId,
    pub user_id: UserId,
    pub member_role: ServerMemberRole,
    pub joined_at: DateTime<Utc>,
}

/// What the access gates need to know about a workspace member in one lookup: their role and,
/// while a timeout is in force, when it ends. Returned by
/// [`crate::server_member::repository::ServerMemberRepository::get_member_standing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberStanding {
    pub role: ServerMemberRole,
    /// `Some(until)` only while the member is timed out (`until` is in the future); expired
    /// timeouts read as `None`.
    pub timed_out_until: Option<DateTime<Utc>>,
}

impl MemberStanding {
    /// A standing with no active timeout.
    pub fn new(role: ServerMemberRole) -> Self {
        Self {
            role,
            timed_out_until: None,
        }
    }
}
