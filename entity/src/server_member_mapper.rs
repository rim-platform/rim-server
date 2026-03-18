use crate::{sea_orm_active_enums, server_member};
use rim_core::server_member::model::{ServerMember, ServerMemberRole};

impl From<server_member::Model> for ServerMember {
    fn from(m: server_member::Model) -> Self {
        let member_role = match m.member_role {
            sea_orm_active_enums::ServerMemberRole::Owner => ServerMemberRole::Owner,
            sea_orm_active_enums::ServerMemberRole::Moderator => ServerMemberRole::Moderator,
            sea_orm_active_enums::ServerMemberRole::Guest => ServerMemberRole::Guest,
        };

        Self {
            id: m.id.into(),
            user_id: m.user_id.into(),
            member_role,
            joined_at: m.joined_at.into(),
            server_id: m.server_id.into(),
        }
    }
}

impl From<ServerMemberRole> for sea_orm_active_enums::ServerMemberRole {
    fn from(value: ServerMemberRole) -> Self {
        match value {
            ServerMemberRole::Owner => sea_orm_active_enums::ServerMemberRole::Owner,
            ServerMemberRole::Guest => sea_orm_active_enums::ServerMemberRole::Guest,
            ServerMemberRole::Moderator => sea_orm_active_enums::ServerMemberRole::Moderator,
        }
    }
}
