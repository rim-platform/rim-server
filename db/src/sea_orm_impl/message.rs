use async_trait::async_trait;
use sea_orm::{ActiveModelTrait, ColumnTrait, DbConn, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set};
use rim_core::repositories::repository::Repository;
use rim_core::repositories::message_repository::MessageRepository;
use crate::impl_repository_for_entity;
use entity::{message};

pub struct SeaOrmMessageRepository {
    db: DbConn,
}

impl SeaOrmMessageRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

type Id = i32;

impl_repository_for_entity!(
    SeaOrmMessageRepository,
    message::Model,
    message::Column,
    Id,
    message::Entity
);

#[async_trait]
impl MessageRepository<message::Model, Id> for SeaOrmMessageRepository {
    async fn create_message(&self, thread_id: Id, parent_message_id: Option<Id>, sent_by: Id, text: &str, has_attachments: bool) -> anyhow::Result<message::Model> {
        let active = message::ActiveModel {
            message_thread_id: Set(thread_id),
            parent_message_id: Set(parent_message_id),
            sent_by: Set(sent_by),
            text: Set(text.to_owned()),
            is_deleted: Set(false),
            has_attachments: Set(has_attachments),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await?;
        Ok(inserted)
    }

    async fn delete_message(&self, message_id: Id) -> anyhow::Result<()> {
        if let Some(msg) = message::Entity::find_by_id(message_id).one(&self.db).await? {
            let mut active: message::ActiveModel = msg.into();

            active.is_deleted = Set(true);

            active.update(&self.db).await?;
        }

        Ok(())
    }

    async fn get_messages_for_thread(&self, thread_id: Id) -> anyhow::Result<Vec<message::Model>> {
        let msgs = message::Entity::find()
            .filter(message::Column::MessageThreadId.eq(thread_id))
            .all(&self.db)
            .await?;
        
        Ok(msgs)
    }

}