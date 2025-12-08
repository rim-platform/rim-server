use std::str::FromStr;
use sea_orm::{ActiveEnum, ColIdx};
use rim_core::server_member::model::{ServerMember, ServerMemberRole};
use crate::{sea_orm_active_enums, server_member};

impl From<server_member::Model> for ServerMember {
    fn from(m: server_member::Model) -> Self {
        Self {
            id: m.id.into(),
            user_id: m.user_id.into(),
            member_role: ServerMemberRole::from_str(m.member_role.into_value().as_str()).unwrap(),
            joined_at: m.joined_at.into(),
            server_id: m.server_id.into(),
        }
    }
}

impl Into<sea_orm_active_enums::ServerMemberRole> for ServerMemberRole {
    fn into(self) -> sea_orm_active_enums::ServerMemberRole {
        match self {
            ServerMemberRole::Owner => sea_orm_active_enums::ServerMemberRole::Owner,
            ServerMemberRole::Guest => sea_orm_active_enums::ServerMemberRole::Guest,
            ServerMemberRole::Moderator => sea_orm_active_enums::ServerMemberRole::Moderator,
        }
    }
}