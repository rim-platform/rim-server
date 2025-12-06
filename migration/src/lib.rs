pub use sea_orm_migration::prelude::*;


pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20251012_135522_create_user::Migration),
            Box::new(m20251029_222439_create_server::Migration),
            Box::new(m20251122_154540_create_conversation::Migration),
            Box::new(m20251123_105135_create_threads_message::Migration),
        ]
    }
}
mod m20251012_135522_create_user;
mod m20251029_222439_create_server;
mod m20251122_154540_create_conversation;
mod m20251123_105135_create_threads_message;
