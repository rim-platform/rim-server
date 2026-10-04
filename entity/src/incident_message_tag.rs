use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "incident_message_tag")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub message_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub incident_id: Uuid,
    #[sea_orm(column_type = "String(StringLen::N(50))")]
    pub phase: String,
    #[sea_orm(column_type = "String(StringLen::N(50))", nullable)]
    pub tag_type: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::message::Entity",
        from = "Column::MessageId",
        to = "super::message::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    Message,
    #[sea_orm(
        belongs_to = "super::incident::Entity",
        from = "Column::IncidentId",
        to = "super::incident::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Incident,
}

impl Related<super::message::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Message.def()
    }
}

impl Related<super::incident::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Incident.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
