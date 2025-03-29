use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.create_table(
            Table::create()
                .table(Channel::Table)
                .if_not_exists()
                .col(ColumnDef::new(Channel::Id).uuid().not_null().primary_key())
                .col(ColumnDef::new(Channel::ServerId).uuid().not_null())
                .col(ColumnDef::new(Channel::Name).string().not_null())
                .col(ColumnDef::new(Channel::Topic).string().null())
                .col(ColumnDef::new(Channel::CreatedAt).timestamp().not_null())
                .col(ColumnDef::new(Channel::UpdatedAt).timestamp().not_null())
                // Foreign key (server)
                .foreign_key(
                    ForeignKey::create()
                        .name("fk-channel-server_id")
                        .from(Channel::Table, Channel::ServerId)
                        .to(Server::Table, Server::Id)
                        .on_delete(ForeignKeyAction::Cascade)
                )
                .to_owned(),
        )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(Channel::Table).to_owned()).await
    }
}

#[derive(DeriveIden)]
enum Channel {
    Table,
    Id,
    ServerId,
    Name,
    Topic,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Server {
    Table,
    Id,
}