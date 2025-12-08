use sea_orm_migration::{prelude::*, schema::*};
use crate::extension::postgres::Type;
use crate::m20251012_135522_create_user::User;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_type(
                Type::create()
                    .as_enum(ServerMemberRole::Table)
                    .values([
                        ServerMemberRole::Owner,
                        ServerMemberRole::Moderator,
                        ServerMemberRole::Guest
                    ])
                    .to_owned()
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(Server::Table)
                    .if_not_exists()
                    .col(uuid_uniq(Server::Id).primary_key())
                    .col(string(Server::Name))
                    .col(
                        timestamp_with_time_zone(Server::CreatedAt)
                            .not_null()
                            .default(Expr::current_timestamp())
                    )
                    .col(
                        timestamp_with_time_zone(Server::UpdatedAt)
                            .not_null()
                            .default(Expr::current_timestamp())
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ServerMember::Table)
                    .if_not_exists()
                    .col(uuid_uniq(ServerMember::Id).primary_key())
                    .col(uuid(ServerMember::ServerId).not_null())
                    .col(uuid(ServerMember::UserId).not_null())
                    .col(enumeration(ServerMember::MemberRole, ServerMemberRole::Table, [
                            ServerMemberRole::Moderator,
                            ServerMemberRole::Guest,
                            ServerMemberRole::Owner,
                        ])
                        .not_null()
                        .default("guest")
                    )
                    .col(
                        timestamp_with_time_zone(ServerMember::JoinedAt)
                            .not_null()
                            .default(Expr::current_timestamp())
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-server-id")
                            .from(ServerMember::Table, ServerMember::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade)
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-user-id")
                            .from(ServerMember::Table, ServerMember::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade)
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Server::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ServerMember::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum Server {
    Table,
    Id,
    Name,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
pub enum ServerMember {
    Table,
    Id,
    MemberRole,
    ServerId,
    UserId,
    JoinedAt,
}

#[derive(DeriveIden)]
pub enum ServerMemberRole {
    #[sea_orm(iden = "server_member_role")]
    Table,
    #[sea_orm(iden = "owner")]
    Owner,
    #[sea_orm(iden = "moderator")]
    Moderator,
    #[sea_orm(iden = "guest")]
    Guest,
}
