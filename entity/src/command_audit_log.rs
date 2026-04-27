use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "command_audit_log")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub actor_user_id: Uuid,
    pub server_id: Uuid,
    #[sea_orm(column_type = "Text")]
    pub command_id: String,
    #[sea_orm(column_type = "String(StringLen::N(64))")]
    pub initiator: String,
    pub args_redacted: Json,
    #[sea_orm(column_type = "String(StringLen::N(32))")]
    pub sensitivity: String,
    pub dangerous: bool,
    #[sea_orm(column_type = "String(StringLen::N(64))")]
    pub confirmation_kind: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub typed_verb: Option<String>,
    #[sea_orm(column_type = "String(StringLen::N(32))")]
    pub outcome: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub error_kind: Option<String>,
    #[sea_orm(column_type = "String(StringLen::N(32))", nullable)]
    pub capability_check_outcome: Option<String>,
    #[sea_orm(column_type = "String(StringLen::N(32))", nullable)]
    pub confirmation_outcome: Option<String>,
    pub parent_audit_id: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
