use sea_orm_migration::prelude::*;
use crate::m20250329_201611_create_member_role_type::{MemberRole, MemberRoleVariants};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.create_table(
            Table::create()
                .table(ChannelMember::Table)
                .if_not_exists()
                .col(ColumnDef::new(ChannelMember::UserId).uuid().not_null())
                .col(ColumnDef::new(ChannelMember::ChannelId).uuid().not_null())
                .col(
                    ColumnDef::new(ChannelMember::Role)
                        .custom(MemberRole)
                        .not_null()
                        .default(MemberRoleVariants::Member.to_string()),
                )
                .col(ColumnDef::new(ChannelMember::JoinedAt).timestamp().not_null())
                .primary_key(
                    Index::create()
                        .col(ChannelMember::UserId)
                        .col(ChannelMember::ChannelId),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk-channel_member-user_id")
                        .from(ChannelMember::Table, ChannelMember::UserId)
                        .to(User::Table, User::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk-channel_member-channel_id")
                        .from(ChannelMember::Table, ChannelMember::ChannelId)
                        .to(Channel::Table, Channel::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        ).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ChannelMember::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ChannelMember {
    Table,
    UserId,
    ChannelId,
    Role,
    JoinedAt,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Channel {
    Table,
    Id,
}
