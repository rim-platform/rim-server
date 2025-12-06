use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait,
    ColumnTrait,
    DbConn,
    EntityTrait,
    QueryFilter,
    QueryOrder,
    Set
};
use rim_core::repositories::repository::Repository;
use rim_core::repositories::message_thread_repository::MessageThreadRepository;
use crate::impl_repository_for_entity;
use entity::{message_thread};
use entity::message_thread::Model;


pub struct SeaOrmMessageThreadRepository {
    pub db: DbConn,
}

impl SeaOrmMessageThreadRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

type Id = i32;

impl_repository_for_entity!(
    SeaOrmMessageThreadRepository,
    message_thread::Model,
    message_thread::Column,
    Id,
    message_thread::Entity
);

#[async_trait]
impl MessageThreadRepository<message_thread::Model, Id> for SeaOrmMessageThreadRepository {
    async fn create_thread(&self, conversation_id: Id, first_message_id: Option<Id>) -> anyhow::Result<Model> {
        let active = message_thread::ActiveModel {
            conversation_id: Set(conversation_id),
            first_message_id: Set(first_message_id),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await?;
        Ok(inserted)
    }

    async fn set_first_message(&self, thread_id: Id, message_id: Id) -> anyhow::Result<()> {
        if let Some(thread) = message_thread::Entity::find_by_id(thread_id).one(&self.db).await? {
            let mut active: message_thread::ActiveModel = thread.into();
            active.first_message_id = Set(Some(message_id));
            active.update(&self.db).await?;
        }

        Ok(())
    }

    async fn get_threads_for_conversation(
        &self,
        conversation_id: Id,
    ) -> anyhow::Result<Vec<message_thread::Model>> {
        let threads = message_thread::Entity::find()
            .filter(message_thread::Column::ConversationId.eq(conversation_id))
            .all(&self.db)
            .await?;
        Ok(threads)
    }
}