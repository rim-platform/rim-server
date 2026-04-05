use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Message::Table)
                    .add_column(
                        ColumnDef::new(Message::ForwardedFromMessageId)
                            .uuid()
                            .null(),
                    )
                    .add_foreign_key(
                        &TableForeignKey::new()
                            .name("fk_message_forwarded_from")
                            .from_tbl(Message::Table)
                            .from_col(Message::ForwardedFromMessageId)
                            .to_tbl(Message::Table)
                            .to_col(Message::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .to_owned(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Message::Table)
                    .drop_foreign_key(Alias::new("fk_message_forwarded_from"))
                    .drop_column(Message::ForwardedFromMessageId)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
pub enum Message {
    Table,
    Id,
    ForwardedFromMessageId,
}
