use sea_orm::entity::prelude::*;

/// A URL that posts to one channel as one bot, stored as the SHA-256 of its token.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "incoming_webhook")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub server_id: Uuid,
    pub conversation_id: Uuid,
    pub bot_user_id: Uuid,
    #[sea_orm(column_type = "Text")]
    pub preset: String,
    #[sea_orm(column_type = "Text", unique)]
    pub token_hash: String,
    pub created_by: Uuid,
    pub created_at: DateTimeWithTimeZone,
    pub last_used_at: Option<DateTimeWithTimeZone>,
    pub revoked_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::bot::Entity",
        from = "Column::BotUserId",
        to = "super::bot::Column::UserId",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Bot,
}

impl Related<super::bot::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Bot.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
