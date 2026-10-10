use sea_orm_migration::prelude::*;

/// Drops the incident module's tables: incidents and their phases and message tags, escalation
/// policies, steps and runs, on-call schedules, rotations and overrides, alert-ingest tokens,
/// the workspace feature toggle and skill profiles. Dependent tables go first.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// The dropped tables, each listed after every table that references it.
const TABLES: &[&str] = &[
    "escalation_run",
    "incident_message_tag",
    "incident_phase",
    "incident",
    "conversation_escalation_policy",
    "escalation_step",
    "escalation_policy",
    "on_call_override",
    "on_call_rotation_participant",
    "on_call_rotation_layer",
    "on_call_schedule",
    "workspace_ingest_token",
    "workspace_feature",
    "user_skill_profile",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for table in TABLES {
            db.execute_unprepared(&format!("DROP TABLE IF EXISTS \"{table}\" CASCADE"))
                .await?;
        }
        Ok(())
    }

    /// Succeeds without recreating the dropped tables; the `incidents-last` tag is the way back.
    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        tracing::warn!(
            "drop_incident_module rolled back: the dropped tables are not recreated; \
             the incidents-last tag is the way back"
        );
        Ok(())
    }
}
