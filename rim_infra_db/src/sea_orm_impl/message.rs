use crate::impl_repository_for_entity;
use async_trait::async_trait;
use entity::message;
use rim_core::message::model::{Message, MessageId, NewMessage};
use rim_core::message::repository::MessageRepository;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::repository::repository::Repository;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, DbConn, EntityTrait, QueryFilter, Set};

pub struct SeaOrmMessageRepository {
    db: DbConn,
}

impl SeaOrmMessageRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

impl_repository_for_entity!(SeaOrmMessageRepository, message::Entity, Message, MessageId);

#[async_trait]
impl MessageRepository for SeaOrmMessageRepository {
    async fn create_message(
        &self,
        new_message: &NewMessage
    ) -> anyhow::Result<Self::Model> {
        let active = message::ActiveModel {
            message_thread_id: Set(new_message.message_thread_id.into()),
            parent_message_id: Set(new_message.parent_message_id.map(Into::into)),
            sent_by: Set(new_message.sent_by.into()),
            text: Set(new_message.text.to_owned()),
            is_deleted: Set(false),
            has_attachments: Set(!new_message.attachments.is_empty()),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await?;
        Ok(inserted.into())
    }

    async fn delete_message(&self, message_id: Self::Id) -> anyhow::Result<()> {
        if let Some(msg) = message::Entity::find_by_id(message_id)
            .one(&self.db)
            .await?
        {
            let mut active: message::ActiveModel = msg.into();

            active.is_deleted = Set(true);

            active.update(&self.db).await?;
        }

        Ok(())
    }

    async fn get_messages_for_thread(
        &self,
        thread_id: MessageThreadId,
    ) -> anyhow::Result<Vec<Self::Model>> {
        let msgs = message::Entity::find()
            .filter(message::Column::MessageThreadId.eq::<Uuid>(thread_id.into()))
            .all(&self.db)
            .await?;

        Ok(msgs.into_iter().map(Into::into).collect())
    }
}
