use crate::m20251012_135522_create_user::User;
use crate::m20251029_222439_create_server::Server;
use crate::m20251122_154540_create_conversation::Conversation;
use sea_orm_migration::{prelude::*, schema::*};

/// Creates `workspace_ingest_token` — the auth mechanism for alert-ingestion webhooks
/// (Track B2, `docs/alerts.md`). Mirrors the `session`/`invite` shape
/// (`m20260212_000001_create_sessions_and_invites`): only a SHA-256 hex hash of the raw
/// token is stored, never the raw token itself.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(WorkspaceIngestToken::Table)
                    .if_not_exists()
                    .col(uuid_uniq(WorkspaceIngestToken::Id).primary_key())
                    .col(uuid(WorkspaceIngestToken::ServerId).not_null())
                    .col(uuid(WorkspaceIngestToken::ConversationId).not_null())
                    .col(uuid(WorkspaceIngestToken::CreatedBy).not_null())
                    .col(
                        string(WorkspaceIngestToken::Label)
                            .not_null()
                            .string_len(128),
                    )
                    .col(
                        string(WorkspaceIngestToken::TokenHash)
                            .not_null()
                            .string_len(128),
                    )
                    .col(
                        string(WorkspaceIngestToken::SeverityDefault)
                            .null()
                            .string_len(20),
                    )
                    .col(
                        timestamp_with_time_zone(WorkspaceIngestToken::CreatedAt)
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(timestamp_with_time_zone(WorkspaceIngestToken::RevokedAt).null())
                    .col(timestamp_with_time_zone(WorkspaceIngestToken::LastUsedAt).null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-workspace-ingest-token-server")
                            .from(WorkspaceIngestToken::Table, WorkspaceIngestToken::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-workspace-ingest-token-conversation")
                            .from(
                                WorkspaceIngestToken::Table,
                                WorkspaceIngestToken::ConversationId,
                            )
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-workspace-ingest-token-created-by")
                            .from(WorkspaceIngestToken::Table, WorkspaceIngestToken::CreatedBy)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .index(
                        Index::create()
                            .name("idx-workspace-ingest-token-hash-uniq")
                            .table(WorkspaceIngestToken::Table)
                            .col(WorkspaceIngestToken::TokenHash)
                            .unique(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-workspace-ingest-token-server")
                    .table(WorkspaceIngestToken::Table)
                    .col(WorkspaceIngestToken::ServerId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(WorkspaceIngestToken::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum WorkspaceIngestToken {
    Table,
    Id,
    ServerId,
    ConversationId,
    CreatedBy,
    Label,
    TokenHash,
    SeverityDefault,
    CreatedAt,
    RevokedAt,
    LastUsedAt,
}
