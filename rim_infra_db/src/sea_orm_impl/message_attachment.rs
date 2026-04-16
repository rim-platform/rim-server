use async_trait::async_trait;
use entity::message_attachment;
use rim_core::message::model::MessageId;
use rim_core::message_attachment::model::{
    MessageAttachment, MessageAttachmentId, NewMessageAttachment,
};
use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_core::repository::error::RepoResult;
use rim_core::repository::error::{RepoError, RepoErrorKind};
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, Condition, EntityTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;
use std::collections::HashMap;

pub struct SeaOrmMessageAttachmentRepository {
    pub db: DbHandle,
}

impl SeaOrmMessageAttachmentRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MessageAttachmentRepository for SeaOrmMessageAttachmentRepository {
    async fn get_by_id(&self, id: MessageAttachmentId) -> RepoResult<Option<MessageAttachment>> {
        let model = message_attachment::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn add_attachment(
        &self,
        attachment: NewMessageAttachment,
    ) -> RepoResult<MessageAttachment> {
        let NewMessageAttachment {
            id,
            message_id,
            storage_url,
            file_name,
            file_size,
            mime_type,
            metadata,
        } = attachment;

        let active = message_attachment::ActiveModel {
            id: Set(id.into()),
            message_id: Set(message_id.into()),
            storage_url: Set(storage_url),
            file_name: Set(file_name),
            file_size: Set(file_size),
            mime_type: Set(mime_type),
            metadata: Set(metadata),
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn get_attachments_for_message(
        &self,
        message_id: MessageId,
    ) -> RepoResult<Vec<MessageAttachment>> {
        let atts = message_attachment::Entity::find()
            .filter(message_attachment::Column::MessageId.eq::<Uuid>(message_id.into()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(atts.into_iter().map(Into::into).collect())
    }

    async fn get_attachments_for_messages(
        &self,
        message_ids: &[MessageId],
    ) -> RepoResult<HashMap<MessageId, Vec<MessageAttachment>>> {
        if message_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let ids = message_ids
            .iter()
            .copied()
            .map(Into::into)
            .collect::<Vec<Uuid>>();
        let atts = message_attachment::Entity::find()
            .filter(message_attachment::Column::MessageId.is_in(ids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        let mut out: HashMap<MessageId, Vec<MessageAttachment>> = HashMap::new();
        for att in atts.into_iter().map(Into::<MessageAttachment>::into) {
            out.entry(att.message_id).or_default().push(att);
        }
        Ok(out)
    }

    async fn update_attachment(
        &self,
        id: MessageAttachmentId,
        storage_url: String,
        metadata: Option<serde_json::Value>,
    ) -> RepoResult<MessageAttachment> {
        let model = message_attachment::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: message_attachment::ActiveModel = model.into();
        active.storage_url = Set(storage_url);
        active.metadata = Set(metadata);

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn delete_attachment(&self, id: MessageAttachmentId) -> RepoResult<()> {
        let model = message_attachment::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        let Some(model) = model else {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        };

        let active: message_attachment::ActiveModel = model.into();
        active.delete(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn find_ids_with_status_before(
        &self,
        statuses: Vec<String>,
        before: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<Vec<MessageAttachmentId>> {
        // Build a condition matching any of the requested metadata status values.
        let mut status_cond = Condition::any();
        for s in &statuses {
            // Postgres JSON: metadata->>'status' = '<s>'
            status_cond = status_cond.add(sea_orm::sea_query::Expr::cust_with_values(
                "metadata->>'status' = $1",
                [sea_orm::Value::from(s.as_str())],
            ));
        }

        let models = message_attachment::Entity::find()
            .filter(status_cond)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        // Filter by UUID v7 encoded timestamp (cheaper in Rust than SQL).
        let cutoff_ts = before.timestamp() as u64;
        let ids = models
            .into_iter()
            .filter(|m| {
                let ts = m.id.get_timestamp().map(|t| t.to_unix().0);
                matches!(ts, Some(secs) if secs < cutoff_ts)
            })
            .map(|m| m.id.into())
            .collect();

        Ok(ids)
    }
}
