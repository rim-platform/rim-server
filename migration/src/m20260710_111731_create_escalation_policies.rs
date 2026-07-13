use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ── escalation_policy table ─────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(EscalationPolicy::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EscalationPolicy::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(EscalationPolicy::ServerId).uuid().not_null())
                    .col(
                        ColumnDef::new(EscalationPolicy::Name)
                            .string_len(200)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EscalationPolicy::CreatedBy)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EscalationPolicy::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(EscalationPolicy::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_escalation_policy_server")
                            .from(EscalationPolicy::Table, EscalationPolicy::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_escalation_policy_created_by")
                            .from(EscalationPolicy::Table, EscalationPolicy::CreatedBy)
                            .to(User::Table, User::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_escalation_policy_server")
                    .table(EscalationPolicy::Table)
                    .col(EscalationPolicy::ServerId)
                    .to_owned(),
            )
            .await?;

        // ── escalation_step table ───────────────────────────────────────────
        // `target_kind` is a varchar discriminator ("user" | "on_call"); exactly one of
        // `target_user_id`/`target_schedule_id` is populated per row (CHECK below, mirroring the
        // application-level invariant on `rim_core::escalation::model::EscalationTarget`).
        // `actions` stores the sorted CSV of `rim_core::escalation::model::EscalationAction`
        // discriminators (e.g. "email,mention_in_channel") — see `encode_actions`/`decode_actions`.
        manager
            .create_table(
                Table::create()
                    .table(EscalationStep::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EscalationStep::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(EscalationStep::PolicyId).uuid().not_null())
                    .col(
                        ColumnDef::new(EscalationStep::Position)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EscalationStep::DelaySeconds)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(EscalationStep::TargetKind)
                            .string_len(20)
                            .not_null(),
                    )
                    .col(ColumnDef::new(EscalationStep::TargetUserId).uuid())
                    .col(ColumnDef::new(EscalationStep::TargetScheduleId).uuid())
                    .col(
                        ColumnDef::new(EscalationStep::Actions)
                            .string_len(200)
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_escalation_step_policy")
                            .from(EscalationStep::Table, EscalationStep::PolicyId)
                            .to(EscalationPolicy::Table, EscalationPolicy::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_escalation_step_target_user")
                            .from(EscalationStep::Table, EscalationStep::TargetUserId)
                            .to(User::Table, User::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_escalation_step_target_schedule")
                            .from(EscalationStep::Table, EscalationStep::TargetScheduleId)
                            .to(OnCallSchedule::Table, OnCallSchedule::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_escalation_step_policy_position")
                    .table(EscalationStep::Table)
                    .col(EscalationStep::PolicyId)
                    .col(EscalationStep::Position)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE escalation_step ADD CONSTRAINT chk_escalation_step_target CHECK ( \
                     (target_kind = 'user' AND target_user_id IS NOT NULL AND target_schedule_id IS NULL) \
                     OR (target_kind = 'on_call' AND target_schedule_id IS NOT NULL AND target_user_id IS NULL) \
                 )",
            )
            .await?;

        // ── conversation_escalation_policy table ────────────────────────────
        // The default escalation policy for incidents declared in a conversation. `server_id` is
        // denormalized (also derivable via `policy_id`) so a lookup by conversation doesn't need a
        // join — mirrors other conversation-scoped one-row-per-conversation tables in this schema.
        manager
            .create_table(
                Table::create()
                    .table(ConversationEscalationPolicy::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationEscalationPolicy::ConversationId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConversationEscalationPolicy::ServerId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationEscalationPolicy::PolicyId)
                            .uuid()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_conversation_escalation_policy_conversation")
                            .from(
                                ConversationEscalationPolicy::Table,
                                ConversationEscalationPolicy::ConversationId,
                            )
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_conversation_escalation_policy_server")
                            .from(
                                ConversationEscalationPolicy::Table,
                                ConversationEscalationPolicy::ServerId,
                            )
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_conversation_escalation_policy_policy")
                            .from(
                                ConversationEscalationPolicy::Table,
                                ConversationEscalationPolicy::PolicyId,
                            )
                            .to(EscalationPolicy::Table, EscalationPolicy::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Reverse FK order: conversation binding / step (leaf) -> policy.
        manager
            .drop_table(
                Table::drop()
                    .table(ConversationEscalationPolicy::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(EscalationStep::Table).to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(EscalationPolicy::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum EscalationPolicy {
    Table,
    Id,
    ServerId,
    Name,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
pub enum EscalationStep {
    Table,
    Id,
    PolicyId,
    Position,
    DelaySeconds,
    TargetKind,
    TargetUserId,
    TargetScheduleId,
    Actions,
}

#[derive(DeriveIden)]
pub enum ConversationEscalationPolicy {
    Table,
    ConversationId,
    ServerId,
    PolicyId,
}

#[derive(DeriveIden)]
pub enum OnCallSchedule {
    Table,
    Id,
}

#[derive(DeriveIden)]
pub enum Conversation {
    Table,
    Id,
}

#[derive(DeriveIden)]
pub enum Server {
    Table,
    Id,
}

#[derive(DeriveIden)]
pub enum User {
    Table,
    Id,
}
