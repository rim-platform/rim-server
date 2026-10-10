use crate::extension::postgres::Type;
use crate::m20251012_135522_create_user::User;
use crate::m20251029_222439_create_server::Server;
use crate::m20251122_154540_create_conversation::Conversation;
use sea_orm_migration::{prelude::*, schema::*};

/// Moderation state.
///
/// - `server_ban` — one row per banned user and workspace, primary key `(server_id, user_id)`.
///   A ban with `expires_at` in the past is ignored; `unban` deletes the row.
/// - `server_member_timeout` — one row per timed-out member, primary key `(server_id, user_id)`.
///   Only rows with `until > now()` take effect; expired rows are ignored and overwritten by the
///   next timeout.
/// - `conversation.slow_mode_seconds` — per-channel slow mode, `0` (off) to `21600` (6 h).
/// - `message_report` + enum `message_report_status` (`open`, `resolved`, `dismissed`) — a
///   member reports a message once (`UNIQUE (message_id, reporter_user_id)`); moderators list and
///   resolve them per workspace (index on `(server_id, status, created_at, id)`).
///
/// The actor columns (`banned_by`, `set_by`, `resolved_by`) are nullable with `ON DELETE SET
/// NULL` so deleting a moderator's account keeps the moderation history.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ServerBan::Table)
                    .if_not_exists()
                    .col(uuid(ServerBan::ServerId).not_null())
                    .col(uuid(ServerBan::UserId).not_null())
                    .col(text_null(ServerBan::Reason))
                    .col(uuid_null(ServerBan::BannedBy))
                    .col(
                        timestamp_with_time_zone(ServerBan::CreatedAt)
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(timestamp_with_time_zone_null(ServerBan::ExpiresAt))
                    .primary_key(
                        Index::create()
                            .name("pk-server-ban")
                            .col(ServerBan::ServerId)
                            .col(ServerBan::UserId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-server-ban-server")
                            .from(ServerBan::Table, ServerBan::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-server-ban-user")
                            .from(ServerBan::Table, ServerBan::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-server-ban-banned-by")
                            .from(ServerBan::Table, ServerBan::BannedBy)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ServerMemberTimeout::Table)
                    .if_not_exists()
                    .col(uuid(ServerMemberTimeout::ServerId).not_null())
                    .col(uuid(ServerMemberTimeout::UserId).not_null())
                    .col(timestamp_with_time_zone(ServerMemberTimeout::Until).not_null())
                    .col(text_null(ServerMemberTimeout::Reason))
                    .col(uuid_null(ServerMemberTimeout::SetBy))
                    .col(
                        timestamp_with_time_zone(ServerMemberTimeout::CreatedAt)
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .primary_key(
                        Index::create()
                            .name("pk-server-member-timeout")
                            .col(ServerMemberTimeout::ServerId)
                            .col(ServerMemberTimeout::UserId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-server-member-timeout-server")
                            .from(ServerMemberTimeout::Table, ServerMemberTimeout::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-server-member-timeout-user")
                            .from(ServerMemberTimeout::Table, ServerMemberTimeout::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-server-member-timeout-set-by")
                            .from(ServerMemberTimeout::Table, ServerMemberTimeout::SetBy)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .add_column(
                        ColumnDef::new(SlowMode::SlowModeSeconds)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE conversation ADD CONSTRAINT conversation_slow_mode_seconds_check \
                 CHECK (slow_mode_seconds BETWEEN 0 AND 21600)",
            )
            .await?;

        manager
            .create_type(
                Type::create()
                    .as_enum(MessageReportStatus::Table)
                    .values([
                        MessageReportStatus::Open,
                        MessageReportStatus::Resolved,
                        MessageReportStatus::Dismissed,
                    ])
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(MessageReport::Table)
                    .if_not_exists()
                    .col(uuid(MessageReport::Id).primary_key())
                    .col(uuid(MessageReport::ServerId).not_null())
                    .col(uuid(MessageReport::ConversationId).not_null())
                    .col(uuid(MessageReport::MessageId).not_null())
                    .col(uuid(MessageReport::ReporterUserId).not_null())
                    .col(text(MessageReport::Reason).not_null())
                    .col(
                        enumeration(
                            MessageReport::Status,
                            MessageReportStatus::Table,
                            [
                                MessageReportStatus::Open,
                                MessageReportStatus::Resolved,
                                MessageReportStatus::Dismissed,
                            ],
                        )
                        .not_null()
                        .default("open"),
                    )
                    .col(uuid_null(MessageReport::ResolvedBy))
                    .col(timestamp_with_time_zone_null(MessageReport::ResolvedAt))
                    .col(
                        timestamp_with_time_zone(MessageReport::CreatedAt)
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-message-report-server")
                            .from(MessageReport::Table, MessageReport::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-message-report-conversation")
                            .from(MessageReport::Table, MessageReport::ConversationId)
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-message-report-message")
                            .from(MessageReport::Table, MessageReport::MessageId)
                            .to(Message::Table, Message::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-message-report-reporter")
                            .from(MessageReport::Table, MessageReport::ReporterUserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-message-report-resolved-by")
                            .from(MessageReport::Table, MessageReport::ResolvedBy)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_message_report_message_reporter_unique")
                    .table(MessageReport::Table)
                    .col(MessageReport::MessageId)
                    .col(MessageReport::ReporterUserId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_message_report_server_status_created")
                    .table(MessageReport::Table)
                    .col(MessageReport::ServerId)
                    .col(MessageReport::Status)
                    .col(MessageReport::CreatedAt)
                    .col(MessageReport::Id)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(MessageReport::Table).to_owned())
            .await?;
        manager
            .drop_type(Type::drop().name(MessageReportStatus::Table).to_owned())
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE conversation DROP CONSTRAINT conversation_slow_mode_seconds_check",
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .drop_column(SlowMode::SlowModeSeconds)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(ServerMemberTimeout::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ServerBan::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ServerBan {
    Table,
    ServerId,
    UserId,
    Reason,
    BannedBy,
    CreatedAt,
    ExpiresAt,
}

#[derive(DeriveIden)]
enum ServerMemberTimeout {
    Table,
    ServerId,
    UserId,
    Until,
    Reason,
    SetBy,
    CreatedAt,
}

/// The `conversation.slow_mode_seconds` column (the shared `Conversation` iden lacks it).
#[derive(DeriveIden)]
enum SlowMode {
    SlowModeSeconds,
}

#[derive(DeriveIden)]
enum MessageReport {
    Table,
    Id,
    ServerId,
    ConversationId,
    MessageId,
    ReporterUserId,
    Reason,
    Status,
    ResolvedBy,
    ResolvedAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum MessageReportStatus {
    #[sea_orm(iden = "message_report_status")]
    Table,
    #[sea_orm(iden = "open")]
    Open,
    #[sea_orm(iden = "resolved")]
    Resolved,
    #[sea_orm(iden = "dismissed")]
    Dismissed,
}

#[derive(DeriveIden)]
enum Message {
    Table,
    Id,
}
