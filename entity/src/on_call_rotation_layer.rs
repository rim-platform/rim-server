use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "on_call_rotation_layer")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub schedule_id: Uuid,
    #[sea_orm(column_type = "String(StringLen::N(200))")]
    pub name: String,
    pub priority: i32,
    #[sea_orm(column_type = "String(StringLen::N(20))")]
    pub kind: String,
    pub interval_seconds: Option<i64>,
    pub handoff_time: Option<Time>,
    pub handoff_weekday: Option<i16>,
    pub anchor_at: DateTimeWithTimeZone,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::on_call_rotation_participant::Entity")]
    OnCallRotationParticipant,
    #[sea_orm(
        belongs_to = "super::on_call_schedule::Entity",
        from = "Column::ScheduleId",
        to = "super::on_call_schedule::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    OnCallSchedule,
}

impl Related<super::on_call_rotation_participant::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OnCallRotationParticipant.def()
    }
}

impl Related<super::on_call_schedule::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::OnCallSchedule.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
