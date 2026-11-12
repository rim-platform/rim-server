use sea_orm::entity::prelude::*;

/// One account's Home layout: the boxes it shows, in order, as JSON.
#[derive(Clone, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "home_layout")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    #[sea_orm(column_type = "JsonBinary")]
    pub layout: Json,
    pub edited_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

/// Leaves out the layout, so a log line or a panic message never carries it.
impl std::fmt::Debug for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Model")
            .field("user_id", &self.user_id)
            .field("edited_at", &self.edited_at)
            .field("updated_at", &self.updated_at)
            .finish_non_exhaustive()
    }
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
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::User.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
