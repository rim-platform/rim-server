use crate::Actor;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AddServerMemberCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub user_id: UserId,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RemoveServerMemberCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub user_id: UserId,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LeaveServerCmd {
    pub actor: Actor,
    pub server_id: ServerId,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GetServerMemberQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub user_id: UserId,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ListServerMembersQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}
