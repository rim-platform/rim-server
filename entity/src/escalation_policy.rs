use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "escalation_policy")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub server_id: Uuid,
    #[sea_orm(column_type = "String(StringLen::N(200))")]
    pub name: String,
    pub created_by: Uuid,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::conversation_escalation_policy::Entity")]
    ConversationEscalationPolicy,
    #[sea_orm(has_many = "super::escalation_run::Entity")]
    EscalationRun,
    #[sea_orm(has_many = "super::escalation_step::Entity")]
    EscalationStep,
    #[sea_orm(has_many = "super::incident::Entity")]
    Incident,
    #[sea_orm(
        belongs_to = "super::server::Entity",
        from = "Column::ServerId",
        to = "super::server::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Server,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::CreatedBy",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    User,
}

impl Related<super::conversation_escalation_policy::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ConversationEscalationPolicy.def()
    }
}

impl Related<super::escalation_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EscalationRun.def()
    }
}

impl Related<super::escalation_step::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EscalationStep.def()
    }
}

impl Related<super::incident::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Incident.def()
    }
}

impl Related<super::server::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Server.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
