use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "on_call_override")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub schedule_id: Uuid,
    pub user_id: Uuid,
    pub start_at: DateTimeWithTimeZone,
    pub end_at: DateTimeWithTimeZone,
    pub created_by: Uuid,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::on_call_schedule::Entity",
        from = "Column::ScheduleId",
        to = "super::on_call_schedule::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    OnCallSchedule,
    // Two FKs to `user` (the overriding participant vs. who created the override) — named
    // explicitly rather than the codegen's disambiguated `User1`/`User2`.
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::UserId",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    OverrideUser,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::CreatedBy",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    CreatedByUser,
}

impl Related<super::on_call_schedule::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OnCallSchedule.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
