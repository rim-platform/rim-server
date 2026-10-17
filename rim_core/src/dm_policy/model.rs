//! Who may start a direct message with a person: the operator's settings for the whole server,
//! each workspace's floor and default for its members, and the person's own choice.

use crate::server::model::ServerId;
use crate::server_member::model::ServerMemberRole;

/// Whether people on other servers, and people outside a recipient's workspaces, may start a
/// DM with this server's people (`RIM_DM_INBOUND`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DmInbound {
    /// No floor: each person's choice applies.
    Open,
    /// Their first message is a request.
    #[default]
    Requests,
    /// Only people on the partner servers (and this one) may, with a request.
    Partners,
    /// Nobody outside a shared workspace may.
    None,
}

impl DmInbound {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Requests => "requests",
            Self::Partners => "partners",
            Self::None => "none",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            "requests" => Some(Self::Requests),
            "partners" => Some(Self::Partners),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// Whether this server's people may start DMs with people on other servers
/// (`RIM_DM_OUTBOUND`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DmOutbound {
    #[default]
    Allowed,
    /// Only with people on the partner servers.
    Partners,
    None,
}

impl DmOutbound {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Partners => "partners",
            Self::None => "none",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allowed" => Some(Self::Allowed),
            "partners" => Some(Self::Partners),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// A workspace's floor for DMs to its members from people outside their workspaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutsideFloor {
    /// No floor.
    #[default]
    None,
    /// At least a request.
    Requests,
    /// Refused.
    Closed,
}

impl OutsideFloor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Requests => "requests",
            Self::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "requests" => Some(Self::Requests),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

/// How a DM from a fellow member of a workspace arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemberDm {
    /// Straight into the conversation list.
    #[default]
    Direct,
    /// As a request.
    Requests,
}

impl MemberDm {
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

/// A person's choice for DMs from people outside their workspaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutsideDm {
    #[default]
    Requests,
    /// Refused.
    None,
}

impl OutsideDm {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requests => "requests",
            Self::None => "none",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "requests" => Some(Self::Requests),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// A workspace's DM rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceDmPolicy {
    pub server_id: ServerId,
    pub outside_floor: OutsideFloor,
    pub member_default: MemberDm,
}

/// One of a user's workspaces with its DM rules and the user's choice for its members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserWorkspaceDm {
    pub server_id: ServerId,
    pub name: String,
    pub outside_floor: OutsideFloor,
    pub member_default: MemberDm,
    /// The user's choice; `None` follows `member_default`.
    pub choice: Option<MemberDm>,
}

impl UserWorkspaceDm {
    /// How a DM from a fellow member arrives for the user.
    pub fn members(&self) -> MemberDm {
        self.choice.unwrap_or(self.member_default)
    }
}

/// A workspace a recipient shares with a sender, with the sender's role in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedWorkspace {
    pub workspace: UserWorkspaceDm,
    pub sender_role: ServerMemberRole,
}
