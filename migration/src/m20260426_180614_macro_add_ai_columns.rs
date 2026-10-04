use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Macro::Table)
                    .add_column(
                        ColumnDef::new(Macro::AiCallable)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .add_column(
                        ColumnDef::new(Macro::NlDescription)
                            .text()
                            .not_null()
                            .default(""),
                    )
                    .add_column(
                        ColumnDef::new(Macro::IntentSamples)
                            .json_binary()
                            .not_null()
                            .default("[]"),
                    )
                    .add_column(
                        ColumnDef::new(Macro::Sensitivity)
                            .string_len(32)
                            .not_null()
                            .default("safe"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Macro::Table)
                    .drop_column(Macro::AiCallable)
                    .drop_column(Macro::NlDescription)
                    .drop_column(Macro::IntentSamples)
                    .drop_column(Macro::Sensitivity)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Macro {
    Table,
    AiCallable,
    NlDescription,
    IntentSamples,
    Sensitivity,
}
