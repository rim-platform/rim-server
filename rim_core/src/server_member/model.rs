use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};
use std::fmt;
use std::str::FromStr;

model_id!(ServerMemberId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerMemberRole {
    Owner,
    Moderator,
    Guest,
}

impl ServerMemberRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServerMemberRole::Owner => "owner",
            ServerMemberRole::Moderator => "moderator",
            ServerMemberRole::Guest => "guest",
        }
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
