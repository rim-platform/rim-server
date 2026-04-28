use crate::m20251012_135522_create_user::User;
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // --- bootstrap_token ---
        manager
            .create_table(
                Table::create()
                    .table(BootstrapToken::Table)
                    .if_not_exists()
                    .col(uuid_uniq(BootstrapToken::Id).primary_key())
                    .col(string(BootstrapToken::Email).not_null().string_len(254))
                    .col(string(BootstrapToken::TokenHash).not_null().string_len(128))
                    .col(
                        timestamp_with_time_zone(BootstrapToken::CreatedAt)
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(timestamp_with_time_zone(BootstrapToken::ExpiresAt).not_null())
                    .col(timestamp_with_time_zone(BootstrapToken::UsedAt).null())
                    .col(uuid(BootstrapToken::ConsumedByUserId).null())
                    .col(timestamp_with_time_zone(BootstrapToken::RevokedAt).null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-bootstrap-token-consumed-by-user")
                            .from(BootstrapToken::Table, BootstrapToken::ConsumedByUserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .index(
                        Index::create()
                            .name("idx-bootstrap-token-hash-uniq")
                            .table(BootstrapToken::Table)
                            .col(BootstrapToken::TokenHash)
                            .unique(),
                    )
                    .to_owned(),
            )
            .await?;

        // --- instance_invite ---
        manager
            .create_table(
                Table::create()
                    .table(InstanceInvite::Table)
                    .if_not_exists()
                    .col(uuid_uniq(InstanceInvite::Id).primary_key())
                    .col(uuid(InstanceInvite::CreatedByUserId).not_null())
                    .col(string(InstanceInvite::TokenHash).not_null().string_len(128))
                    .col(
                        timestamp_with_time_zone(InstanceInvite::CreatedAt)
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(timestamp_with_time_zone(InstanceInvite::ExpiresAt).not_null())
                    .col(
                        integer(InstanceInvite::MaxUses)
                            .not_null()
                            .default(1)
                            .check(
                                Expr::col(InstanceInvite::MaxUses)
                                    .gte(1)
                                    .and(Expr::col(InstanceInvite::MaxUses).lte(50)),
                            ),
                    )
                    .col(
                        integer(InstanceInvite::UseCount)
                            .not_null()
                            .default(0)
                            .check(Expr::col(InstanceInvite::UseCount).gte(0)),
                    )
                    .col(timestamp_with_time_zone(InstanceInvite::RevokedAt).null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-instance-invite-created-by-user")
                            .from(InstanceInvite::Table, InstanceInvite::CreatedByUserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .index(
                        Index::create()
                            .name("idx-instance-invite-hash-uniq")
                            .table(InstanceInvite::Table)
                            .col(InstanceInvite::TokenHash)
                            .unique(),
                    )
                    .index(
                        Index::create()
                            .name("idx-instance-invite-active")
                            .table(InstanceInvite::Table)
                            .col(InstanceInvite::RevokedAt)
                            .col(InstanceInvite::ExpiresAt),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(InstanceInvite::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(BootstrapToken::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum BootstrapToken {
    Table,
    Id,
    Email,
    TokenHash,
    CreatedAt,
    ExpiresAt,
    UsedAt,
    ConsumedByUserId,
    RevokedAt,
}

#[derive(DeriveIden)]
enum InstanceInvite {
    Table,
    Id,
    CreatedByUserId,
    TokenHash,
    CreatedAt,
    ExpiresAt,
    MaxUses,
    UseCount,
    RevokedAt,
}
