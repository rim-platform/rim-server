use crate::model_id;
use crate::server::model::ServerId;
use crate::user::model::UserId;
use chrono::{DateTime, Utc};

model_id!(ServerMemberId);

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

impl std::str::FromStr for ServerMemberRole {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "owner" => Ok(ServerMemberRole::Owner),
            "moderator" => Ok(ServerMemberRole::Moderator),
            "guest" => Ok(ServerMemberRole::Guest),
            other => Err(anyhow::anyhow!("Unknown role: {other}")),
        }
    }
}

pub struct ServerMember {
    pub id: ServerMemberId,
    pub server_id: ServerId,
    pub user_id: UserId,
    pub member_role: ServerMemberRole,
    pub joined_at: DateTime<Utc>,
}
