use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "incident")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub server_id: Uuid,
    pub conversation_id: Uuid,
    pub started_by: Uuid,
    #[sea_orm(column_type = "String(StringLen::N(20))")]
    pub status: String,
    #[sea_orm(column_type = "String(StringLen::N(20))", nullable)]
    pub severity: Option<String>,
    #[sea_orm(column_type = "String(StringLen::N(200))")]
    pub title: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub original_topic: Option<String>,
    pub started_at: DateTimeWithTimeZone,
    pub mitigated_at: Option<DateTimeWithTimeZone>,
    pub resolved_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(column_type = "Text", nullable)]
    pub postmortem_draft: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    #[sea_orm(column_type = "String(StringLen::N(128))", nullable)]
    pub alert_fingerprint: Option<String>,
    #[sea_orm(column_type = "String(StringLen::N(32))")]
    pub source: String,
    pub escalation_policy_id: Option<Uuid>,
    pub acknowledged_at: Option<DateTimeWithTimeZone>,
    pub acknowledged_by: Option<Uuid>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::server::Entity",
        from = "Column::ServerId",
        to = "super::server::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Server,
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
        from = "Column::StartedBy",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    User,
    // Two more FKs to `user` (ack) and one to `escalation_policy` — named explicitly rather
    // than the codegen's disambiguated `User1`/`User2`.
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::AcknowledgedBy",
        to = "super::user::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    AcknowledgedByUser,
    #[sea_orm(
        belongs_to = "super::escalation_policy::Entity",
        from = "Column::EscalationPolicyId",
        to = "super::escalation_policy::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    EscalationPolicy,
    #[sea_orm(has_one = "super::escalation_run::Entity")]
    EscalationRun,
    #[sea_orm(has_many = "super::incident_phase::Entity")]
    IncidentPhases,
    #[sea_orm(has_many = "super::incident_message_tag::Entity")]
    IncidentMessageTags,
}

impl Related<super::server::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Server.def()
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

impl Related<super::incident_phase::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::IncidentPhases.def()
    }
}

impl Related<super::incident_message_tag::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::IncidentMessageTags.def()
    }
}

impl Related<super::escalation_policy::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EscalationPolicy.def()
    }
}

impl Related<super::escalation_run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::EscalationRun.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
