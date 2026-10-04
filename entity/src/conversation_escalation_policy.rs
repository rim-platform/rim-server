use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "conversation_escalation_policy")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub conversation_id: Uuid,
    pub server_id: Uuid,
    pub policy_id: Uuid,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::conversation::Entity",
        from = "Column::ConversationId",
        to = "super::conversation::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Conversation,
    #[sea_orm(
        belongs_to = "super::escalation_policy::Entity",
        from = "Column::PolicyId",
        to = "super::escalation_policy::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    EscalationPolicy,
    #[sea_orm(
        belongs_to = "super::server::Entity",
        from = "Column::ServerId",
        to = "super::server::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Server,
}

impl Related<super::conversation::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Conversation.def()
    }
}

impl Related<super::escalation_policy::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EscalationPolicy.def()
    }
}

impl Related<super::server::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Server.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
