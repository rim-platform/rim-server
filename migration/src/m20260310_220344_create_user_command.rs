use sea_orm_migration::prelude::*;

use crate::m20251012_135522_create_user::User;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UserCommand::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserCommand::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(UserCommand::OwnerUserId).uuid().not_null())
                    .col(ColumnDef::new(UserCommand::CommandKey).string().not_null())
                    .col(ColumnDef::new(UserCommand::Title).string().not_null())
                    .col(ColumnDef::new(UserCommand::Description).string().not_null())
                    .col(
                        ColumnDef::new(UserCommand::StepsJson)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserCommand::IsEnabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(UserCommand::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(UserCommand::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-user_command-owner_user_id")
                            .from(UserCommand::Table, UserCommand::OwnerUserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-user_command-owner_user_id")
                    .table(UserCommand::Table)
                    .col(UserCommand::OwnerUserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-user_command-owner_user_id-command_key")
                    .table(UserCommand::Table)
                    .col(UserCommand::OwnerUserId)
                    .col(UserCommand::CommandKey)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserCommand::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum UserCommand {
    Table,
    Id,
    OwnerUserId,
    CommandKey,
    Title,
    Description,
    StepsJson,
    IsEnabled,
    CreatedAt,
    UpdatedAt,
}
