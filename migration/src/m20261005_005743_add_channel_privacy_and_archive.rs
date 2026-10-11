use crate::m20251012_135522_create_user::User;
use sea_orm_migration::prelude::*;

/// Private channels and archiving.
///
/// - `conversation.is_private` — only meaningful for channels; private channels are visible to
///   their `conversation_member` rows alone. Fixed at creation.
/// - `conversation.archived_at` — set while the conversation is archived (read-only).
/// - `conversation.created_by` — the creator, `NULL` for rows created before this migration (no
///   back-fill is possible) or whose user was deleted.
/// - Index `(server_id, conversation_type, is_private, archived_at)` for the channel listings.
///
/// **Data step:** until now channels had no real membership, so "joined channels" would start
/// empty. Every current workspace member becomes a member of every existing channel in their
/// workspace; existing rows are kept (`ON CONFLICT` on the
/// `idx_conversation_member_conversation_user_unique` index). Ids are UUIDv7 built in SQL from
/// the clock and `gen_random_uuid()` (Postgres 13+ has no native v7 generator before 18).
/// The data step: every workspace member joins every channel of their workspace, keeping
/// existing rows. Public so the `rim_infra_db` db-tests can run it against seeded data.
pub const CHANNEL_MEMBERSHIP_BACKFILL_SQL: &str = "\
    INSERT INTO conversation_member (id, conversation_id, user_id, joined_at) \
    SELECT encode(set_bit(set_bit(overlay(uuid_send(gen_random_uuid()) \
             placing substring(int8send(floor(extract(epoch FROM clock_timestamp()) \
             * 1000)::bigint) FROM 3) FROM 1 FOR 6), 52, 1), 53, 1), 'hex')::uuid, \
           c.id, sm.user_id, now() \
    FROM conversation c \
    JOIN server_member sm ON sm.server_id = c.server_id \
    WHERE c.conversation_type = 'channel' \
    ON CONFLICT (conversation_id, user_id) DO NOTHING";

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .add_column(
                        ColumnDef::new(Conversation::IsPrivate)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .add_column(
                        ColumnDef::new(Conversation::ArchivedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_column(ColumnDef::new(Conversation::CreatedBy).uuid().null())
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("fk-conversation-created-by")
                            .from_tbl(Conversation::Table)
                            .from_col(Conversation::CreatedBy)
                            .to_tbl(User::Table)
                            .to_col(User::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_server_type_private_archived")
                    .table(Conversation::Table)
                    .col(Conversation::ServerId)
                    .col(Conversation::ConversationType)
                    .col(Conversation::IsPrivate)
                    .col(Conversation::ArchivedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(CHANNEL_MEMBERSHIP_BACKFILL_SQL)
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // The data step is not reverted: the membership rows it added stay valid channel members.
        manager
            .drop_index(
                Index::drop()
                    .name("idx_conversation_server_type_private_archived")
                    .table(Conversation::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .drop_foreign_key(Alias::new("fk-conversation-created-by"))
                    .drop_column(Conversation::CreatedBy)
                    .drop_column(Conversation::ArchivedAt)
                    .drop_column(Conversation::IsPrivate)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
#[allow(clippy::enum_variant_names)]
enum Conversation {
    Table,
    ServerId,
    ConversationType,
    IsPrivate,
    ArchivedAt,
    CreatedBy,
}
