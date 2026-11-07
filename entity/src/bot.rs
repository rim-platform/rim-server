use sea_orm::entity::prelude::*;

/// A bot account's workspace and disclosure; its name lives on its `user` row.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "bot")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    pub server_id: Uuid,
    #[sea_orm(column_type = "Text")]
    pub run_by: String,
    #[sea_orm(column_type = "Text")]
    pub reads_go_to: String,
    pub created_by: Uuid,
    pub created_at: DateTimeWithTimeZone,
    pub deactivated_at: Option<DateTimeWithTimeZone>,
    pub member_joins_since: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    User,
    #[sea_orm(
        belongs_to = "super::server::Entity",
        from = "Column::ServerId",
        to = "super::server::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Server,
    #[sea_orm(has_many = "super::bot_token::Entity")]
    BotToken,
}

impl Related<super::server::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Server.def()
    }
}

impl Related<super::bot_token::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::BotToken.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
