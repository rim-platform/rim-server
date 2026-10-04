use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "escalation_step")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub policy_id: Uuid,
    pub position: i32,
    pub delay_seconds: i64,
    #[sea_orm(column_type = "String(StringLen::N(20))")]
    pub target_kind: String,
    pub target_user_id: Option<Uuid>,
    pub target_schedule_id: Option<Uuid>,
    #[sea_orm(column_type = "String(StringLen::N(200))")]
    pub actions: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::escalation_policy::Entity",
        from = "Column::PolicyId",
        to = "super::escalation_policy::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    EscalationPolicy,
    #[sea_orm(
        belongs_to = "super::on_call_schedule::Entity",
        from = "Column::TargetScheduleId",
        to = "super::on_call_schedule::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    TargetSchedule,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::TargetUserId",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    TargetUser,
}

impl Related<super::escalation_policy::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EscalationPolicy.def()
    }
}

impl Related<super::on_call_schedule::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::TargetSchedule.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::TargetUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
