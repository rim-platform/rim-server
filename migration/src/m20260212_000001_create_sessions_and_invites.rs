use crate::m20251012_135522_create_user::User;
use crate::m20251029_222439_create_server::Server;
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Session::Table)
                    .if_not_exists()
                    .col(uuid_uniq(Session::Id).primary_key())
                    .col(uuid(Session::UserId).not_null())
                    .col(string(Session::TokenHash).not_null().string_len(128))
                    .col(
                        timestamp_with_time_zone(Session::CreatedAt)
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(timestamp_with_time_zone(Session::ExpiresAt).not_null())
                    .col(timestamp_with_time_zone(Session::LastSeenAt).null())
                    .col(timestamp_with_time_zone(Session::RevokedAt).null())
                    .col(string(Session::UserAgent).null().string_len(512))
                    .col(string(Session::Ip).null().string_len(64))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-session-user")
                            .from(Session::Table, Session::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .index(
                        Index::create()
                            .name("idx-session-token-hash-uniq")
                            .table(Session::Table)
                            .col(Session::TokenHash)
                            .unique(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Invite::Table)
                    .if_not_exists()
                    .col(uuid_uniq(Invite::Id).primary_key())
                    .col(uuid(Invite::ServerId).not_null())
                    .col(uuid(Invite::CreatedByUserId).not_null())
                    .col(string(Invite::TokenHash).not_null().string_len(128))
                    .col(
                        timestamp_with_time_zone(Invite::CreatedAt)
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(timestamp_with_time_zone(Invite::ExpiresAt).null())
                    .col(integer(Invite::MaxUses).not_null().default(1))
                    .col(integer(Invite::UseCount).not_null().default(0))
                    .col(timestamp_with_time_zone(Invite::RevokedAt).null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-invite-server")
                            .from(Invite::Table, Invite::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-invite-created-by-user")
                            .from(Invite::Table, Invite::CreatedByUserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .index(
                        Index::create()
                            .name("idx-invite-token-hash-uniq")
                            .table(Invite::Table)
                            .col(Invite::TokenHash)
                            .unique(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Invite::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Session::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Session {
    Table,
    Id,
    UserId,
    TokenHash,
    CreatedAt,
    ExpiresAt,
    LastSeenAt,
    RevokedAt,
    UserAgent,
    Ip,
}

#[derive(DeriveIden)]
enum Invite {
    Table,
    Id,
    ServerId,
    CreatedByUserId,
    TokenHash,
    CreatedAt,
    ExpiresAt,
    MaxUses,
    UseCount,
    RevokedAt,
}
