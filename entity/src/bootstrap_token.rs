//! `SeaORM` Entity, manually maintained.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "bootstrap_token")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub email: String,
    pub token_hash: String,
    pub created_at: DateTimeWithTimeZone,
    pub expires_at: DateTimeWithTimeZone,
    pub used_at: Option<DateTimeWithTimeZone>,
    pub consumed_by_user_id: Option<Uuid>,
    pub revoked_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::ConsumedByUserId",
        to = "super::user::Column::Id",
        on_update = "Cascade",
        on_delete = "SetNull"
    )]
    ConsumedByUser,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::ConsumedByUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
