//! `SeaORM` Entity, manually maintained.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "instance_invite")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub created_by_user_id: Option<Uuid>,
    pub token_hash: String,
    pub created_at: DateTimeWithTimeZone,
    pub expires_at: DateTimeWithTimeZone,
    pub max_uses: i32,
    pub use_count: i32,
    pub revoked_at: Option<DateTimeWithTimeZone>,
    pub email: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::CreatedByUserId",
        to = "super::user::Column::Id",
        on_update = "Cascade",
        on_delete = "Restrict"
    )]
    CreatedByUser,
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::CreatedByUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
