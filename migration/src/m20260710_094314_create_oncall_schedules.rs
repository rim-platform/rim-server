use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ── on_call_schedule table ─────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(OnCallSchedule::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OnCallSchedule::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(OnCallSchedule::ServerId).uuid().not_null())
                    .col(
                        ColumnDef::new(OnCallSchedule::Name)
                            .string_len(200)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OnCallSchedule::Timezone)
                            .string_len(64)
                            .not_null(),
                    )
                    .col(ColumnDef::new(OnCallSchedule::CreatedBy).uuid().not_null())
                    .col(
                        ColumnDef::new(OnCallSchedule::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(OnCallSchedule::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oncall_schedule_server")
                            .from(OnCallSchedule::Table, OnCallSchedule::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oncall_schedule_created_by")
                            .from(OnCallSchedule::Table, OnCallSchedule::CreatedBy)
                            .to(User::Table, User::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_oncall_schedule_server")
                    .table(OnCallSchedule::Table)
                    .col(OnCallSchedule::ServerId)
                    .to_owned(),
            )
            .await?;

        // ── on_call_rotation_layer table ───────────────────────────────────
        // `kind` is stored as a varchar discriminator ("daily" | "weekly" | "custom"); the
        // per-kind fields are all nullable and only the ones relevant to `kind` are populated
        // (application-level invariant, documented in rim_core::oncall::model::RotationKind).
        manager
            .create_table(
                Table::create()
                    .table(OnCallRotationLayer::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OnCallRotationLayer::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OnCallRotationLayer::ScheduleId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OnCallRotationLayer::Name)
                            .string_len(200)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OnCallRotationLayer::Priority)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OnCallRotationLayer::Kind)
                            .string_len(20)
                            .not_null(),
                    )
                    .col(ColumnDef::new(OnCallRotationLayer::IntervalSeconds).big_integer())
                    .col(ColumnDef::new(OnCallRotationLayer::HandoffTime).time())
                    .col(ColumnDef::new(OnCallRotationLayer::HandoffWeekday).small_integer())
                    .col(
                        ColumnDef::new(OnCallRotationLayer::AnchorAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OnCallRotationLayer::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oncall_rotation_layer_schedule")
                            .from(OnCallRotationLayer::Table, OnCallRotationLayer::ScheduleId)
                            .to(OnCallSchedule::Table, OnCallSchedule::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_oncall_rotation_layer_schedule")
                    .table(OnCallRotationLayer::Table)
                    .col(OnCallRotationLayer::ScheduleId)
                    .to_owned(),
            )
            .await?;

        // ── on_call_rotation_participant table ─────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(OnCallRotationParticipant::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OnCallRotationParticipant::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(OnCallRotationParticipant::LayerId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OnCallRotationParticipant::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OnCallRotationParticipant::Position)
                            .integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oncall_rotation_participant_layer")
                            .from(
                                OnCallRotationParticipant::Table,
                                OnCallRotationParticipant::LayerId,
                            )
                            .to(OnCallRotationLayer::Table, OnCallRotationLayer::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oncall_rotation_participant_user")
                            .from(
                                OnCallRotationParticipant::Table,
                                OnCallRotationParticipant::UserId,
                            )
                            .to(User::Table, User::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_oncall_rotation_participant_layer_position")
                    .table(OnCallRotationParticipant::Table)
                    .col(OnCallRotationParticipant::LayerId)
                    .col(OnCallRotationParticipant::Position)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // ── on_call_override table ─────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(OnCallOverride::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(OnCallOverride::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(OnCallOverride::ScheduleId).uuid().not_null())
                    .col(ColumnDef::new(OnCallOverride::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(OnCallOverride::StartAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(OnCallOverride::EndAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(OnCallOverride::CreatedBy).uuid().not_null())
                    .col(
                        ColumnDef::new(OnCallOverride::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oncall_override_schedule")
                            .from(OnCallOverride::Table, OnCallOverride::ScheduleId)
                            .to(OnCallSchedule::Table, OnCallSchedule::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oncall_override_user")
                            .from(OnCallOverride::Table, OnCallOverride::UserId)
                            .to(User::Table, User::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_oncall_override_created_by")
                            .from(OnCallOverride::Table, OnCallOverride::CreatedBy)
                            .to(User::Table, User::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_oncall_override_schedule_range")
                    .table(OnCallOverride::Table)
                    .col(OnCallOverride::ScheduleId)
                    .col(OnCallOverride::StartAt)
                    .col(OnCallOverride::EndAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Reverse FK order: override/participant (leaf) -> layer -> schedule.
        manager
            .drop_table(Table::drop().table(OnCallOverride::Table).to_owned())
            .await?;

        manager
            .drop_table(
                Table::drop()
                    .table(OnCallRotationParticipant::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(OnCallRotationLayer::Table).to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(OnCallSchedule::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum OnCallSchedule {
    Table,
    Id,
    ServerId,
    Name,
    Timezone,
    CreatedBy,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
pub enum OnCallRotationLayer {
    Table,
    Id,
    ScheduleId,
    Name,
    Priority,
    Kind,
    IntervalSeconds,
    HandoffTime,
    HandoffWeekday,
    AnchorAt,
    CreatedAt,
}

#[derive(DeriveIden)]
pub enum OnCallRotationParticipant {
    Table,
    Id,
    LayerId,
    UserId,
    Position,
}

#[derive(DeriveIden)]
pub enum OnCallOverride {
    Table,
    Id,
    ScheduleId,
    UserId,
    StartAt,
    EndAt,
    CreatedBy,
    CreatedAt,
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
