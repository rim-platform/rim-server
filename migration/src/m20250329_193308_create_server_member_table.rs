use sea_orm_migration::prelude::*;
use crate::m20250329_201611_create_member_role_type::{MemberRole, MemberRoleVariants};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.create_table(
            Table::create()
                .table(ServerMember::Table)
                .if_not_exists()
                .col(ColumnDef::new(ServerMember::UserId).uuid().not_null())
                .col(ColumnDef::new(ServerMember::ServerId).uuid().not_null())
                .col(
                    ColumnDef::new(ServerMember::Role)
                        .custom(MemberRole)
                        .not_null()
                        .default(MemberRoleVariants::Member.to_string()),
                )
                .col(ColumnDef::new(ServerMember::JoinedAt).timestamp().not_null())
                .primary_key(
                    Index::create()
                        .col(ServerMember::UserId)
                        .col(ServerMember::ServerId),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk-server_member-user_id")
                        .from(ServerMember::Table, ServerMember::UserId)
                        .to(User::Table, User::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk-server_member-server_id")
                        .from(ServerMember::Table, ServerMember::ServerId)
                        .to(Server::Table, Server::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        ).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ServerMember::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ServerMember {
    Table,
    UserId,
    ServerId,
    Role,
    JoinedAt,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Server {
    Table,
    Id,
}
