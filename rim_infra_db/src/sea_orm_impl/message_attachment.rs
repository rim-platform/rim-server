use std::collections::HashMap;

use async_trait::async_trait;
use entity::{message, message_attachment};
use rim_core::message::model::MessageId;
use rim_core::message_attachment::model::{
    AttachmentUsage, MessageAttachment, MessageAttachmentId, NewMessageAttachment,
};
use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_core::repository::error::RepoResult;
use rim_core::repository::error::{RepoError, RepoErrorKind};
use rim_core::server::model::ServerId;
use sea_orm::prelude::Uuid;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseTransaction, DbBackend,
    EntityTrait, FromQueryResult, QueryFilter, QuerySelect, Set, Statement,
};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmMessageAttachmentRepository {
    pub db: DbHandle,
}

impl SeaOrmMessageAttachmentRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

/// Locks a parent message before changing its attachment rows.
async fn lock_message(txn: &DatabaseTransaction, id: Uuid) -> RepoResult<message::Model> {
    message::Entity::find_by_id(id)
        .lock_exclusive()
        .one(txn)
        .await
        .map_err(map_db_err)?
        .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))
}

/// Advances the locked message in its conversation's change feed without marking a text edit.
async fn advance_message(txn: &DatabaseTransaction, parent: &message::Model) -> RepoResult<()> {
    let next = txn
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO message_change_seq AS s (conversation_id, last_version) VALUES ($1, 1) \
         ON CONFLICT (conversation_id) DO UPDATE SET last_version = s.last_version + 1 \
         RETURNING last_version",
            [parent.conversation_id.into()],
        ))
        .await
        .map_err(map_db_err)?
        .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
    let version: i64 = next.try_get("", "last_version").map_err(map_db_err)?;
    message::Entity::update_many()
        .col_expr(message::Column::Version, Expr::value(version))
        .filter(message::Column::Id.eq(parent.id))
        .exec(txn)
        .await
        .map_err(map_db_err)?;
    Ok(())
}

fn transaction_error(error: sea_orm::TransactionError<RepoError>) -> RepoError {
    match error {
        sea_orm::TransactionError::Connection(error) => map_db_err(error),
        sea_orm::TransactionError::Transaction(error) => error,
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
            blob_sha256,
        } = attachment;

        let active = message_attachment::ActiveModel {
            id: Set(id.into()),
            message_id: Set(message_id.into()),
            storage_url: Set(storage_url),
            file_name: Set(file_name),
            file_size: Set(file_size),
            mime_type: Set(mime_type),
            metadata: Set(metadata),
            blob_sha256: Set(blob_sha256),
        };

        let inserted = self
            .db
            .transaction(|txn| {
                Box::pin(async move {
                    let parent = lock_message(txn, message_id.into()).await?;
                    let inserted = active.insert(txn).await.map_err(map_db_err)?;
                    advance_message(txn, &parent).await?;
                    Ok(inserted)
                })
            })
            .await
            .map_err(transaction_error)?;
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
        let updated = self
            .db
            .transaction(|txn| {
                Box::pin(async move {
                    let model = message_attachment::Entity::find_by_id(id)
                        .one(txn)
                        .await
                        .map_err(map_db_err)?
                        .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
                    let parent = lock_message(txn, model.message_id).await?;
                    let mut active: message_attachment::ActiveModel = model.into();
                    active.storage_url = Set(storage_url);
                    active.metadata = Set(metadata);
                    let updated = active.update(txn).await.map_err(map_db_err)?;
                    advance_message(txn, &parent).await?;
                    Ok(updated)
                })
            })
            .await
            .map_err(transaction_error)?;
        Ok(updated.into())
    }

    async fn try_complete_upload(
        &self,
        id: MessageAttachmentId,
        expected_storage_url: String,
        reserved_since: chrono::DateTime<chrono::Utc>,
        storage_url: String,
        file_size: i64,
        metadata: Option<serde_json::Value>,
    ) -> RepoResult<Option<MessageAttachment>> {
        let updated = self
            .db
            .transaction(|txn| {
                Box::pin(async move {
                    let Some(model) = message_attachment::Entity::find_by_id(id)
                        .one(txn)
                        .await
                        .map_err(map_db_err)?
                    else {
                        return Ok(None);
                    };
                    let parent = lock_message(txn, model.message_id).await?;
                    let updated = message_attachment::Entity::update_many()
                        .col_expr(
                            message_attachment::Column::StorageUrl,
                            Expr::value(storage_url),
                        )
                        .col_expr(message_attachment::Column::FileSize, Expr::value(file_size))
                        .col_expr(message_attachment::Column::Metadata, Expr::value(metadata))
                        .filter(message_attachment::Column::Id.eq(Uuid::from(id)))
                        .filter(message_attachment::Column::StorageUrl.eq(expected_storage_url))
                        .filter(message_attachment::Column::Id.gte(v7_lower_bound(reserved_since)))
                        .exec_with_returning(txn)
                        .await
                        .map_err(map_db_err)?;
                    let updated = updated.into_iter().next();
                    if updated.is_some() {
                        advance_message(txn, &parent).await?;
                    }
                    Ok(updated)
                })
            })
            .await
            .map_err(transaction_error)?;
        Ok(updated.map(Into::into))
    }

    async fn delete_attachment(&self, id: MessageAttachmentId) -> RepoResult<()> {
        self.db
            .transaction(|txn| {
                Box::pin(async move {
                    let model = message_attachment::Entity::find_by_id(id)
                        .one(txn)
                        .await
                        .map_err(map_db_err)?
                        .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
                    let parent = lock_message(txn, model.message_id).await?;
                    let active: message_attachment::ActiveModel = model.into();
                    active.delete(txn).await.map_err(map_db_err)?;
                    advance_message(txn, &parent).await?;
                    Ok(())
                })
            })
            .await
            .map_err(transaction_error)
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

    async fn usage_for_server(
        &self,
        server_id: ServerId,
        pending_since: chrono::DateTime<chrono::Utc>,
        max_file_size: i64,
    ) -> RepoResult<AttachmentUsage> {
        #[derive(FromQueryResult)]
        struct UsageRow {
            total_bytes: i64,
            count: i64,
        }

        let row = UsageRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT COALESCE(SUM(GREATEST(LEAST(a.file_size, $3), 0)), 0)::bigint AS total_bytes,
                      COUNT(*)::bigint AS count
                 FROM message_attachment a
                 JOIN message m ON m.id = a.message_id
                 JOIN conversation c ON c.id = m.conversation_id
                WHERE c.server_id = $1
                  AND (a.storage_url NOT LIKE 'pending://%'
                       OR (a.metadata->>'status' = 'pending_upload' AND a.id >= $2))"#,
            [
                Uuid::from(server_id).into(),
                v7_lower_bound(pending_since).into(),
                max_file_size.into(),
            ],
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;

        Ok(
            row.map_or_else(AttachmentUsage::default, |r| AttachmentUsage {
                total_bytes: u64::try_from(r.total_bytes).unwrap_or(0),
                count: u64::try_from(r.count).unwrap_or(0),
            }),
        )
    }

    async fn find_stored_by_blob_sha256(
        &self,
        server_id: ServerId,
        blob_sha256: Vec<u8>,
    ) -> RepoResult<Option<MessageAttachment>> {
        let model = message_attachment::Entity::find()
            .from_raw_sql(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"SELECT a.* FROM message_attachment a
                 JOIN message m ON m.id = a.message_id
                 JOIN conversation c ON c.id = m.conversation_id
                WHERE c.server_id = $1
                  AND a.blob_sha256 = $2
                  AND a.storage_url NOT LIKE 'pending://%'
                ORDER BY a.id
                LIMIT 1"#,
                [Uuid::from(server_id).into(), blob_sha256.into()],
            ))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }
}

/// The smallest UUID v7 for instant `at`. Attachment ids are v7 and Postgres orders `uuid` by
/// bytes, so `id >= v7_lower_bound(t)` selects attachments created at or after `t`.
fn v7_lower_bound(at: chrono::DateTime<chrono::Utc>) -> Uuid {
    let millis = u64::try_from(at.timestamp_millis()).unwrap_or(0);
    let mut bytes = [0u8; 16];
    // 48-bit big-endian Unix milliseconds, then version 7 and the RFC 4122 variant; every
    // random bit stays zero.
    bytes[..6].copy_from_slice(&millis.to_be_bytes()[2..]);
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v7_lower_bound_orders_below_ids_from_the_same_millisecond() {
        let at = chrono::Utc::now();
        let bound = v7_lower_bound(at);
        assert_eq!(bound.get_version_num(), 7);
        let id: Uuid = MessageAttachmentId::new().into();
        assert!(
            bound <= id,
            "an id created now must sort at or above the bound"
        );
        let later = v7_lower_bound(at + chrono::Duration::milliseconds(1));
        assert!(bound < later);
    }
}
