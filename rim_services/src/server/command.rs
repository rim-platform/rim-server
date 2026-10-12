use crate::Actor;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
pub struct CreateServerCmd {
    pub actor: Actor,
    pub name: String,
}

#[derive(Debug, Clone, Copy)]
pub struct GetServerQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

#[derive(Debug, Clone, Copy)]
pub struct ListMyServersQuery {
    pub actor: Actor,
}

/// Hand the workspace's ownership to another member.
#[derive(Debug, Clone, Copy)]
pub struct TransferWorkspaceOwnershipCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    /// The member who becomes the owner.
    pub user_id: UserId,
}

/// Delete the workspace and everything in it.
#[derive(Debug, Clone)]
pub struct DeleteWorkspaceCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    /// The workspace's name, typed again to confirm.
    pub confirm_name: String,
}
