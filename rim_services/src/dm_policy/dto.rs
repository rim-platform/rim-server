use rim_core::dm_policy::model::{DmInbound, DmOutbound, MemberDm, OutsideDm, OutsideFloor};
use rim_core::server::model::ServerId;

/// The operator's DM settings for the whole server: `RIM_DM_INBOUND`, `RIM_DM_OUTBOUND` and
/// `RIM_DM_PARTNERS`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DmOperatorPolicy {
    pub inbound: DmInbound,
    pub outbound: DmOutbound,
    /// Server ids `partners` names.
    pub partners: Vec<String>,
}

impl DmOperatorPolicy {
    fn is_partner(&self, server_id: &str) -> bool {
        self.partners.iter().any(|p| p == server_id)
    }

    /// Whether DMs from people on `server_id`, another server, may arrive at all.
    pub fn admits_inbound(&self, server_id: &str) -> bool {
        match self.inbound {
            DmInbound::Open | DmInbound::Requests => true,
            DmInbound::Partners => self.is_partner(server_id),
            DmInbound::None => false,
        }
    }

    /// Whether this server's people may start DMs with people on `server_id`, another server.
    pub fn admits_outbound(&self, server_id: &str) -> bool {
        match self.outbound {
            DmOutbound::Allowed => true,
            DmOutbound::Partners => self.is_partner(server_id),
            DmOutbound::None => false,
        }
    }
}

/// Where the floor that limits a person's choice comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DmFloorSourceDto {
    /// The operator's `RIM_DM_INBOUND`.
    Operator,
    /// A workspace the person is a member of.
    Workspace {
        workspace_id: ServerId,
        name: String,
    },
}

/// A person's choice for the members of one of their workspaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceDmSettingDto {
    pub workspace_id: ServerId,
    /// The person's choice, or the workspace's default when they made none.
    pub members: MemberDm,
    pub member_default: MemberDm,
}

/// Who may start a DM with a person, as they chose and as it applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MyDmSettingsDto {
    /// Their choice for DMs from outside their workspaces.
    pub outside: OutsideDm,
    pub workspaces: Vec<WorkspaceDmSettingDto>,
    /// What applies to DMs from outside their workspaces: their choice, or `None` when a floor
    /// refuses them.
    pub effective_outside: OutsideDm,
    /// The floor that refuses DMs from outside; `None` when no floor does.
    pub outside_floor_from: Option<DmFloorSourceDto>,
}

/// A workspace's DM rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceDmPolicyDto {
    pub workspace_id: ServerId,
    pub outside_floor: OutsideFloor,
    pub member_default: MemberDm,
}
