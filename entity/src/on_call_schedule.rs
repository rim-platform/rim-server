use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "on_call_schedule")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub server_id: Uuid,
    #[sea_orm(column_type = "String(StringLen::N(200))")]
    pub name: String,
    #[sea_orm(column_type = "String(StringLen::N(64))")]
    pub timezone: String,
    pub created_by: Uuid,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::on_call_override::Entity")]
    OnCallOverride,
    #[sea_orm(has_many = "super::on_call_rotation_layer::Entity")]
    OnCallRotationLayer,
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

impl Related<super::on_call_override::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OnCallOverride.def()
    }
}

impl Related<super::on_call_rotation_layer::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OnCallRotationLayer.def()
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
