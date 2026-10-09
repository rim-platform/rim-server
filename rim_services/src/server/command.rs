use crate::Actor;
use rim_core::server::model::ServerId;

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
