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
            Box::new(m20260122_120000_add_conversation_last_activity_at::Migration),
            Box::new(m20260212_000001_create_sessions_and_invites::Migration),
            Box::new(m20260212_000002_expand_user_password_hash::Migration),
            Box::new(m20260216_230500_fix_user_last_login_at_not_null::Migration),
            Box::new(m20260222_214356_add_conversation_main_thread_id::Migration),
            Box::new(m20260223_220000_make_session_revoked_last_seen_nullable::Migration),
            Box::new(m20260308_210831_add_conversation_owned_messages::Migration),
            Box::new(m20260308_213715_remove_legacy_thread_columns::Migration),
            Box::new(m20260308_215130_add_unique_thread_root_message::Migration),
            Box::new(m20260308_224258_add_membership_uniqueness::Migration),
        ]
    }
}
mod m20251012_135522_create_user;
mod m20251029_222439_create_server;
mod m20251122_154540_create_conversation;
mod m20251123_105135_create_threads_message;
mod m20260122_120000_add_conversation_last_activity_at;
mod m20260212_000001_create_sessions_and_invites;
mod m20260212_000002_expand_user_password_hash;
mod m20260216_230500_fix_user_last_login_at_not_null;
mod m20260222_214356_add_conversation_main_thread_id;
mod m20260223_220000_make_session_revoked_last_seen_nullable;
mod m20260308_210831_add_conversation_owned_messages;
mod m20260308_213715_remove_legacy_thread_columns;
mod m20260308_215130_add_unique_thread_root_message;
mod m20260308_224258_add_membership_uniqueness;
