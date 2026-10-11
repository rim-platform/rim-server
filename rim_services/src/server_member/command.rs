use crate::Actor;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone, Copy)]
pub struct GetServerMemberQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub user_id: UserId,
}

#[derive(Debug, Clone, Copy)]
pub struct ListServerMembersQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}

#[derive(Debug, Clone, Copy)]
pub struct GetServerOwnerQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}
