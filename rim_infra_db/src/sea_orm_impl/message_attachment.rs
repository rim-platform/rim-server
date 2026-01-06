use crate::impl_repository_for_entity;
use async_trait::async_trait;
use entity::message_attachment;
use rim_core::message::model::MessageId;
use rim_core::message_attachment::model::{MessageAttachment, MessageAttachmentId};
use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_core::repository::repository::Repository;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, DbConn, EntityTrait, QueryFilter, Set};

pub struct SeaOrmMessageAttachmentRepository {
    pub db: DbConn,
}

impl SeaOrmMessageAttachmentRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}
impl_repository_for_entity!(
    SeaOrmMessageAttachmentRepository,
    message_attachment::Entity,
    MessageAttachment,
    MessageAttachmentId
);

#[async_trait]
impl MessageAttachmentRepository for SeaOrmMessageAttachmentRepository {
    async fn add_attachment(
        &self,
        message_id: MessageId,
        storage_url: &str,
        file_name: &str,
        file_size: i64,
        mime_type: &str,
        metadata: Option<sea_orm::prelude::Json>,
    ) -> anyhow::Result<Self::Model> {
        let active = message_attachment::ActiveModel {
            message_id: Set(message_id.into()),
            storage_url: Set(storage_url.to_owned()),
            file_name: Set(file_name.to_owned()),
            file_size: Set(file_size),
            mime_type: Set(mime_type.to_owned()),
            metadata: Set(metadata),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await?;
        Ok(inserted.into())
    }

    async fn get_attachments_for_message(
        &self,
        message_id: MessageId,
    ) -> anyhow::Result<Vec<Self::Model>> {
        let atts = message_attachment::Entity::find()
            .filter(message_attachment::Column::MessageId.eq::<Uuid>(message_id.into()))
            .all(&self.db)
            .await?;
        Ok(atts.into_iter().map(Into::into).collect())
    }
}
