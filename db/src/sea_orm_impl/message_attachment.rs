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
use rim_core::repositories::message_attachment_repository::MessageAttachmentRepository;
use crate::impl_repository_for_entity;
use entity::{message_attachment};
use entity::message_attachment::Model;

pub struct SeaOrmMessageAttachmentRepository {
    pub db: DbConn,
}

impl SeaOrmMessageAttachmentRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

type Id = i32;

impl_repository_for_entity!(
    SeaOrmMessageAttachmentRepository,
    message_attachment::Model,
    message_attachment::Column,
    Id,
    message_attachment::Entity
);

#[async_trait]
impl MessageAttachmentRepository<message_attachment::Model, Id> for SeaOrmMessageAttachmentRepository {
    async fn add_attachment(
        &self,
        message_id: Id,
        storage_url: &str,
        file_name: &str,
        file_size: i64,
        mime_type: &str,
        metadata: Option<sea_orm::prelude::Json>
    ) -> anyhow::Result<Model> {
        let active = message_attachment::ActiveModel {
            message_id: Set(message_id),
            storage_url: Set(storage_url.to_owned()),
            file_name: Set(file_name.to_owned()),
            file_size: Set(file_size),
            mime_type: Set(mime_type.to_owned()),
            metadata: Set(metadata),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await?;
        Ok(inserted)
    }

    async fn get_attachments_for_message(&self, message_id: Id) -> anyhow::Result<Vec<Model>> {
        let atts = message_attachment::Entity::find()
            .filter(message_attachment::Column::MessageId.eq(message_id))
            .all(&self.db)
            .await?;
        Ok(atts)
    }
}