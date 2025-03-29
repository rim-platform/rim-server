use sea_orm_migration::prelude::*;
use sea_orm::{EnumIter, Iterable};
use sea_orm_migration::prelude::extension::postgres::Type;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_type(
                Type::create()
                    .as_enum(MemberRole)
                    .values(MemberRoleVariants::iter())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_type(Type::drop().name(MemberRole).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub struct MemberRole;

#[derive(DeriveIden, EnumIter)]
pub enum MemberRoleVariants {
    #[sea_orm(iden = "member")]
    Member,
    #[sea_orm(iden = "moderator")]
    Moderator,
    #[sea_orm(iden = "admin")]
    Admin,
}