use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// P6-M1: widen the `outcome` CHECK constraint on `command_audit_log` to
/// include all 7 variants now emitted by the service layer.
///
/// The original constraint in `m20260425_000001_create_command_audit_log` only
/// listed 4 values (`success | error | confirmation_required | permission_denied`),
/// which caused constraint violations for `rate_limited`, `step_error`, and
/// `macro_partial_failure`.
///
/// The `down` migration restores the original 4-value constraint so the
/// rollback path is clean.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        // Drop the old constraint first (name is from the original migration).
        conn.execute_unprepared(
            "ALTER TABLE command_audit_log \
             DROP CONSTRAINT IF EXISTS command_audit_log_outcome_check",
        )
        .await?;

        // Re-add with the full 7-value set matching AuditOutcome::as_str().
        conn.execute_unprepared(
            "ALTER TABLE command_audit_log \
             ADD CONSTRAINT command_audit_log_outcome_check \
             CHECK (outcome IN (\
               'success', \
               'error', \
               'confirmation_required', \
               'permission_denied', \
               'rate_limited', \
               'step_error', \
               'macro_partial_failure'\
             ))",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        conn.execute_unprepared(
            "ALTER TABLE command_audit_log \
             DROP CONSTRAINT IF EXISTS command_audit_log_outcome_check",
        )
        .await?;

        // Restore the original 4-value constraint.
        conn.execute_unprepared(
            "ALTER TABLE command_audit_log \
             ADD CONSTRAINT command_audit_log_outcome_check \
             CHECK (outcome IN ('success', 'error', 'confirmation_required', 'permission_denied'))",
        )
        .await?;

        Ok(())
    }
}
