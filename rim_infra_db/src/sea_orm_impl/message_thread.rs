use crate::impl_repository_for_entity;
use async_trait::async_trait;
use entity::message_thread;
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message_thread::model::{MessageThread, MessageThreadId};
use rim_core::message_thread::repository::MessageThreadRepository;
use rim_core::repository::repository::Repository;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, DbConn, EntityTrait, QueryFilter, Set};

pub struct SeaOrmMessageThreadRepository {
    pub db: DbConn,
}

impl SeaOrmMessageThreadRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

impl_repository_for_entity!(
    SeaOrmMessageThreadRepository,
    message_thread::Entity,
    MessageThread,
    MessageThreadId
);

#[async_trait]
impl MessageThreadRepository for SeaOrmMessageThreadRepository {
    async fn create_thread(
        &self,
        conversation_id: ConversationId,
        first_message_id: Option<MessageId>,
    ) -> anyhow::Result<Self::Model> {
        let active = message_thread::ActiveModel {
            conversation_id: Set(conversation_id.into()),
            first_message_id: Set(first_message_id.map(Into::into)),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await?;
        Ok(inserted.into())
    }

    async fn set_first_message(
        &self,
        thread_id: MessageThreadId,
        message_id: MessageId,
    ) -> anyhow::Result<()> {
        if let Some(thread) = message_thread::Entity::find_by_id(thread_id)
            .one(&self.db)
            .await?
        {
            let mut active: message_thread::ActiveModel = thread.into();
            active.first_message_id = Set(Some(message_id.into()));
            active.update(&self.db).await?;
        }

        Ok(())
    }

    async fn get_threads_for_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> anyhow::Result<Vec<Self::Model>> {
        let threads = message_thread::Entity::find()
            .filter(message_thread::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .all(&self.db)
            .await?;
        Ok(threads.into_iter().map(Into::into).collect())
    }
}
