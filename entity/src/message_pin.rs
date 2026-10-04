//! `SeaORM` Entity for message_pin table

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "message_pin")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub message_id: Uuid,
    pub conversation_id: Uuid,
    pub pinned_by: Uuid,
    pub pinned_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::message::Entity",
        from = "Column::MessageId",
        to = "super::message::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Message,
    #[sea_orm(
        belongs_to = "super::conversation::Entity",
        from = "Column::ConversationId",
        to = "super::conversation::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Conversation,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::PinnedBy",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    User,
}

impl Related<super::message::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Message.def()
    }
}

impl Related<super::conversation::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Conversation.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
