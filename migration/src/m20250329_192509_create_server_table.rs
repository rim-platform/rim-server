use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.create_table(
            Table::create()
                .table(Server::Table)
                .if_not_exists()
                .col(ColumnDef::new(Server::Id).uuid().not_null().primary_key())
                .col(ColumnDef::new(Server::Name).string().not_null())
                .col(ColumnDef::new(Server::Description).string().null())
                .col(ColumnDef::new(Server::OwnerId).uuid().not_null())
                .col(ColumnDef::new(Server::CreatedAt).timestamp().not_null())
                .col(ColumnDef::new(Server::UpdatedAt).timestamp().not_null())
                .foreign_key(
                    ForeignKey::create()
                        .name("fk-server-owner_id")
                        .from(Server::Table, Server::OwnerId)
                        .to(User::Table, User::Id)
                        .on_delete(ForeignKeyAction::Cascade)
                )
                .to_owned(),
        ).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(Server::Table).to_owned()).await
    }
}

#[derive(DeriveIden)]
enum Server {
    Table,
    Id,
    Name,
    Description,
    OwnerId,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
}
