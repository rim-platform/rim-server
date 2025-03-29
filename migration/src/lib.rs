pub use sea_orm_migration::prelude::*;

mod m20250329_201611_create_member_role_type;
mod m20250329_190924_create_user_table;
mod m20250329_192509_create_server_table;
mod m20250329_192931_create_channel_table;
mod m20250329_192935_create_message_table;
mod m20250329_183922_create_email_verification_token_table;
mod m20250329_183934_create_password_reset_token_table;
mod m20250329_193308_create_server_member_table;
mod m20250329_193424_create_channel_member_table;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20250329_201611_create_member_role_type::Migration),
            Box::new(m20250329_190924_create_user_table::Migration),
            Box::new(m20250329_192509_create_server_table::Migration),
            Box::new(m20250329_192931_create_channel_table::Migration),
            Box::new(m20250329_192935_create_message_table::Migration),
            Box::new(m20250329_183922_create_email_verification_token_table::Migration),
            Box::new(m20250329_183934_create_password_reset_token_table::Migration),
            Box::new(m20250329_193308_create_server_member_table::Migration),
            Box::new(m20250329_193424_create_channel_member_table::Migration),
        ]
    }
}
