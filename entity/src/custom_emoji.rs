use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "custom_emoji")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub server_id: Uuid,
    #[sea_orm(column_type = "String(StringLen::N(64))")]
    pub name: String,
    #[sea_orm(column_type = "Text")]
    pub image_url: String,
    pub uploaded_by: Uuid,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
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
        from = "Column::UploadedBy",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    User,
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
