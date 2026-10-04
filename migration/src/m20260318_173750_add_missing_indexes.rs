use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // 1. Unique index on user.username -- prevents duplicate usernames
        //    and speeds up login lookups.
        manager
            .create_index(
                Index::create()
                    .name("idx_user_username_unique")
                    .table(User::Table)
                    .col(User::Username)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // 2. Index on server_member.user_id -- speeds up get_servers_for_user
        //    which filters by user_id alone. The existing unique index is on
        //    (server_id, user_id) which doesn't help user_id-only lookups.
        manager
            .create_index(
                Index::create()
                    .name("idx_server_member_user_id")
                    .table(ServerMember::Table)
                    .col(ServerMember::UserId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_server_member_user_id")
                    .table(ServerMember::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_user_username_unique")
                    .table(User::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum User {
    Table,
    Username,
}

#[derive(DeriveIden)]
enum ServerMember {
    Table,
    UserId,
}
