use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ── incident table ─────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(Incident::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Incident::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Incident::ServerId).uuid().not_null())
                    .col(ColumnDef::new(Incident::ConversationId).uuid().not_null())
                    .col(ColumnDef::new(Incident::StartedBy).uuid().not_null())
                    .col(
                        ColumnDef::new(Incident::Status)
                            .string_len(20)
                            .not_null()
                            .default("active"),
                    )
                    .col(ColumnDef::new(Incident::Severity).string_len(20))
                    .col(
                        ColumnDef::new(Incident::Title)
                            .string_len(200)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Incident::OriginalTopic).text())
                    .col(
                        ColumnDef::new(Incident::StartedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Incident::MitigatedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(Incident::ResolvedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(Incident::PostmortemDraft).text())
                    .col(
                        ColumnDef::new(Incident::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Incident::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_incident_server")
                            .from(Incident::Table, Incident::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_incident_conversation")
                            .from(Incident::Table, Incident::ConversationId)
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_incident_started_by")
                            .from(Incident::Table, Incident::StartedBy)
                            .to(User::Table, User::Id),
                    )
                    .to_owned(),
            )
            .await?;

        // Partial unique index: only one active incident per conversation
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX idx_incident_active_conversation \
                 ON incident (conversation_id) \
                 WHERE status = 'active'",
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_incident_server")
                    .table(Incident::Table)
                    .col(Incident::ServerId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_incident_conversation")
                    .table(Incident::Table)
                    .col(Incident::ConversationId)
                    .to_owned(),
            )
            .await?;

        // ── incident_phase table ───────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(IncidentPhase::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(IncidentPhase::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(IncidentPhase::IncidentId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(IncidentPhase::Phase)
                            .string_len(50)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(IncidentPhase::StartedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(IncidentPhase::EndedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(IncidentPhase::Note).text())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_incident_phase_incident")
                            .from(IncidentPhase::Table, IncidentPhase::IncidentId)
                            .to(Incident::Table, Incident::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_incident_phase_incident")
                    .table(IncidentPhase::Table)
                    .col(IncidentPhase::IncidentId)
                    .to_owned(),
            )
            .await?;

        // ── incident_message_tag table ─────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(IncidentMessageTag::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(IncidentMessageTag::MessageId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(IncidentMessageTag::IncidentId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(IncidentMessageTag::Phase)
                            .string_len(50)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(IncidentMessageTag::TagType)
                            .string_len(50),
                    )
                    .primary_key(
                        Index::create()
                            .col(IncidentMessageTag::MessageId)
                            .col(IncidentMessageTag::IncidentId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_incident_msg_tag_message")
                            .from(IncidentMessageTag::Table, IncidentMessageTag::MessageId)
                            .to(Message::Table, Message::Id),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_incident_msg_tag_incident")
                            .from(IncidentMessageTag::Table, IncidentMessageTag::IncidentId)
                            .to(Incident::Table, Incident::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_incident_msg_tag_incident")
                    .table(IncidentMessageTag::Table)
                    .col(IncidentMessageTag::IncidentId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(IncidentMessageTag::Table).to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(IncidentPhase::Table).to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(Incident::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum Incident {
    Table,
    Id,
    ServerId,
    ConversationId,
    StartedBy,
    Status,
    Severity,
    Title,
    OriginalTopic,
    StartedAt,
    MitigatedAt,
    ResolvedAt,
    PostmortemDraft,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
pub enum IncidentPhase {
    Table,
    Id,
    IncidentId,
    Phase,
    StartedAt,
    EndedAt,
    Note,
}

#[derive(DeriveIden)]
pub enum IncidentMessageTag {
    Table,
    MessageId,
    IncidentId,
    Phase,
    TagType,
}

#[derive(DeriveIden)]
pub enum Server {
    Table,
    Id,
}

#[derive(DeriveIden)]
pub enum Conversation {
    Table,
    Id,
}

#[derive(DeriveIden)]
pub enum User {
    Table,
    Id,
}

#[derive(DeriveIden)]
pub enum Message {
    Table,
    Id,
}
