use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(LinkPreview::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(LinkPreview::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(LinkPreview::MessageId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(LinkPreview::Url).text().not_null())
                    .col(ColumnDef::new(LinkPreview::Title).text())
                    .col(ColumnDef::new(LinkPreview::Description).text())
                    .col(ColumnDef::new(LinkPreview::ImageUrl).text())
                    .col(ColumnDef::new(LinkPreview::SiteName).text())
                    .col(
                        ColumnDef::new(LinkPreview::FetchedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_link_preview_message")
                            .from(LinkPreview::Table, LinkPreview::MessageId)
                            .to(Message::Table, Message::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_link_preview_message")
                    .table(LinkPreview::Table)
                    .col(LinkPreview::MessageId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_link_preview_message_url")
                    .table(LinkPreview::Table)
                    .col(LinkPreview::MessageId)
                    .col(LinkPreview::Url)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_link_preview_message_url")
                    .table(LinkPreview::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_link_preview_message")
                    .table(LinkPreview::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(LinkPreview::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum LinkPreview {
    Table,
    Id,
    MessageId,
    Url,
    Title,
    Description,
    ImageUrl,
    SiteName,
    FetchedAt,
}

#[derive(DeriveIden)]
pub enum Message {
    Table,
    Id,
}
