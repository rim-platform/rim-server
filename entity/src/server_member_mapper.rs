use crate::{sea_orm_active_enums, server_member};
use rim_core::server_member::model::{ServerMember, ServerMemberRole};

impl From<server_member::Model> for ServerMember {
    fn from(m: server_member::Model) -> Self {
        Self {
            id: m.id.into(),
            user_id: m.user_id.into(),
            member_role: m.member_role.into(),
            joined_at: m.joined_at.into(),
            server_id: m.server_id.into(),
        }
    }
}

impl From<sea_orm_active_enums::ServerMemberRole> for ServerMemberRole {
    fn from(value: sea_orm_active_enums::ServerMemberRole) -> Self {
        match value {
            sea_orm_active_enums::ServerMemberRole::Owner => ServerMemberRole::Owner,
            sea_orm_active_enums::ServerMemberRole::Moderator => ServerMemberRole::Moderator,
            sea_orm_active_enums::ServerMemberRole::Member => ServerMemberRole::Member,
            sea_orm_active_enums::ServerMemberRole::Guest => ServerMemberRole::Guest,
        }
    }
}

impl From<ServerMemberRole> for sea_orm_active_enums::ServerMemberRole {
    fn from(value: ServerMemberRole) -> Self {
        match value {
            ServerMemberRole::Owner => sea_orm_active_enums::ServerMemberRole::Owner,
            ServerMemberRole::Moderator => sea_orm_active_enums::ServerMemberRole::Moderator,
            ServerMemberRole::Member => sea_orm_active_enums::ServerMemberRole::Member,
            ServerMemberRole::Guest => sea_orm_active_enums::ServerMemberRole::Guest,
        }
    }
}
