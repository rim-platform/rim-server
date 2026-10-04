use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UserSkillProfile::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserSkillProfile::UserId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(UserSkillProfile::FeaturesUsed)
                            .json_binary()
                            .not_null()
                            .default(Expr::val("{}")),
                    )
                    .col(
                        ColumnDef::new(UserSkillProfile::SkillTier)
                            .string_len(20)
                            .not_null()
                            .default("beginner"),
                    )
                    .col(
                        ColumnDef::new(UserSkillProfile::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_user_skill_profile_user")
                            .from(UserSkillProfile::Table, UserSkillProfile::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UserSkillProfile::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum UserSkillProfile {
    Table,
    UserId,
    FeaturesUsed,
    SkillTier,
    UpdatedAt,
}

#[derive(DeriveIden)]
pub enum User {
    Table,
    Id,
}
