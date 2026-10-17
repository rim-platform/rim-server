//! GraphQL types of who may start a DM with a person: the operator's settings, a workspace's
//! rules and the person's choice.

use async_graphql::*;
use rim_core::dm_policy::model::{
    DmInbound as CoreDmInbound, DmOutbound as CoreDmOutbound, MemberDm, OutsideDm, OutsideFloor,
};
use rim_services::dm_policy::dto::{
    DmFloorSourceDto, MyDmSettingsDto, WorkspaceDmPolicyDto, WorkspaceDmSettingDto,
};

/// DMs from people on other servers, and from people here outside the recipient's workspaces
/// (`RIM_DM_INBOUND`).
#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum DmInbound {
    /// No floor; each person's choice applies.
    Open,
    /// The first message is a request.
    Requests,
    /// Only people on the partner servers and this one, with a request.
    Partners,
    /// Nobody outside a shared workspace.
    None,
}

impl From<CoreDmInbound> for DmInbound {
    fn from(value: CoreDmInbound) -> Self {
        match value {
            CoreDmInbound::Open => Self::Open,
            CoreDmInbound::Requests => Self::Requests,
            CoreDmInbound::Partners => Self::Partners,
            CoreDmInbound::None => Self::None,
        }
    }
}

/// DMs people here start with people on other servers (`RIM_DM_OUTBOUND`).
#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum DmOutbound {
    Allowed,
    /// Only with people on the partner servers.
    Partners,
    None,
}

impl From<CoreDmOutbound> for DmOutbound {
    fn from(value: CoreDmOutbound) -> Self {
        match value {
            CoreDmOutbound::Allowed => Self::Allowed,
            CoreDmOutbound::Partners => Self::Partners,
            CoreDmOutbound::None => Self::None,
        }
    }
}

/// A workspace's floor for DMs to its members from people outside their workspaces.
#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum DmOutsideFloor {
    /// No floor.
    None,
    /// At least a request.
    Requests,
    /// Refused.
    Closed,
}

impl From<OutsideFloor> for DmOutsideFloor {
    fn from(value: OutsideFloor) -> Self {
        match value {
            OutsideFloor::None => Self::None,
            OutsideFloor::Requests => Self::Requests,
            OutsideFloor::Closed => Self::Closed,
        }
    }
}

impl From<DmOutsideFloor> for OutsideFloor {
    fn from(value: DmOutsideFloor) -> Self {
        match value {
            DmOutsideFloor::None => Self::None,
            DmOutsideFloor::Requests => Self::Requests,
            DmOutsideFloor::Closed => Self::Closed,
        }
    }
}

/// How a DM from a fellow member of a workspace arrives.
#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum DmMembers {
    /// Straight into the conversation list.
    Direct,
    /// As a request.
    Requests,
}

impl From<MemberDm> for DmMembers {
    fn from(value: MemberDm) -> Self {
        match value {
            MemberDm::Direct => Self::Direct,
            MemberDm::Requests => Self::Requests,
        }
    }
}

impl From<DmMembers> for MemberDm {
    fn from(value: DmMembers) -> Self {
        match value {
            DmMembers::Direct => Self::Direct,
            DmMembers::Requests => Self::Requests,
        }
    }
}

/// A person's choice for DMs from people outside their workspaces.
#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum DmOutside {
    /// The first message is a request.
    Requests,
    /// Refused.
    None,
}

impl From<OutsideDm> for DmOutside {
    fn from(value: OutsideDm) -> Self {
        match value {
            OutsideDm::Requests => Self::Requests,
            OutsideDm::None => Self::None,
        }
    }
}

impl From<DmOutside> for OutsideDm {
    fn from(value: DmOutside) -> Self {
        match value {
            DmOutside::Requests => Self::Requests,
            DmOutside::None => Self::None,
        }
    }
}

/// Who set a floor.
#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum DmFloorSourceKind {
    /// The server's operator (`RIM_DM_INBOUND`).
    Operator,
    /// A workspace's owner.
    Workspace,
}

/// Where the floor that refuses DMs from outside comes from.
#[derive(SimpleObject, Clone, Debug)]
pub struct DmFloorSource {
    pub kind: DmFloorSourceKind,
    /// The workspace, for `WORKSPACE`.
    pub workspace_id: Option<ID>,
    pub workspace_name: Option<String>,
}

impl From<DmFloorSourceDto> for DmFloorSource {
    fn from(dto: DmFloorSourceDto) -> Self {
        match dto {
            DmFloorSourceDto::Operator => Self {
                kind: DmFloorSourceKind::Operator,
                workspace_id: None,
                workspace_name: None,
            },
            DmFloorSourceDto::Workspace { workspace_id, name } => Self {
                kind: DmFloorSourceKind::Workspace,
                workspace_id: Some(ID(workspace_id.to_string())),
                workspace_name: Some(name),
            },
        }
    }
}

/// The caller's choice for DMs from the members of one of their workspaces.
#[derive(SimpleObject, Clone, Debug)]
pub struct WorkspaceDmSetting {
    pub workspace_id: ID,
    /// Their choice, or the workspace's default when they made none.
    pub members: DmMembers,
    /// The workspace's default.
    pub member_default: DmMembers,
}

impl From<WorkspaceDmSettingDto> for WorkspaceDmSetting {
    fn from(dto: WorkspaceDmSettingDto) -> Self {
        Self {
            workspace_id: ID(dto.workspace_id.to_string()),
            members: dto.members.into(),
            member_default: dto.member_default.into(),
        }
    }
}

/// Who may start a DM with the caller, as they chose and as it applies.
#[derive(SimpleObject, Clone, Debug)]
pub struct MyDmSettings {
    /// Their choice for DMs from outside their workspaces.
    pub outside: DmOutside,
    /// Their choice for each of their workspaces' members.
    pub workspaces: Vec<WorkspaceDmSetting>,
    /// What applies to DMs from outside: their choice, or `NONE` under a floor that refuses
    /// them.
    pub effective_outside: DmOutside,
    /// The floor that refuses DMs from outside; null when none does.
    pub outside_floor_from: Option<DmFloorSource>,
}

impl From<MyDmSettingsDto> for MyDmSettings {
    fn from(dto: MyDmSettingsDto) -> Self {
        Self {
            outside: dto.outside.into(),
            workspaces: dto.workspaces.into_iter().map(Into::into).collect(),
            effective_outside: dto.effective_outside.into(),
            outside_floor_from: dto.outside_floor_from.map(Into::into),
        }
    }
}

/// A workspace's DM rules.
#[derive(SimpleObject, Clone, Debug)]
pub struct WorkspaceDmPolicy {
    pub workspace_id: ID,
    /// The floor for DMs to its members from outside their workspaces.
    pub outside_floor: DmOutsideFloor,
    /// How a DM from a fellow member arrives unless the recipient chose.
    pub member_default: DmMembers,
}

impl From<WorkspaceDmPolicyDto> for WorkspaceDmPolicy {
    fn from(dto: WorkspaceDmPolicyDto) -> Self {
        Self {
            workspace_id: ID(dto.workspace_id.to_string()),
            outside_floor: dto.outside_floor.into(),
            member_default: dto.member_default.into(),
        }
    }
}
