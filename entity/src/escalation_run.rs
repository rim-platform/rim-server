use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "escalation_run")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub incident_id: Uuid,
    pub policy_id: Uuid,
    pub current_step_index: i32,
    pub next_fire_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(column_type = "String(StringLen::N(20))")]
    pub status: String,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
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
        belongs_to = "super::incident::Entity",
        from = "Column::IncidentId",
        to = "super::incident::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Incident,
}

impl Related<super::escalation_policy::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EscalationPolicy.def()
    }
}

impl Related<super::incident::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Incident.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
