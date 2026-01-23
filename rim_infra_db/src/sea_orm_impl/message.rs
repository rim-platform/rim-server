use async_trait::async_trait;
use entity::message;
use rim_core::message::model::{Message, MessageId, NewMessage};
use rim_core::message::repository::{MessageCursor, MessageRepository};
use rim_core::message_thread::model::MessageThreadId;
use rim_core::repository::error::RepoResult;
use rim_core::repository::error::{RepoError, RepoErrorKind};
use rim_core::repository::pagination::{Page, PageDir, SortOrder};
use sea_orm::QuerySelect;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DbConn, EntityTrait, QueryFilter, QueryOrder, Set,
};

use crate::error::map_db_err;

pub struct SeaOrmMessageRepository {
    db: DbConn,
}

impl SeaOrmMessageRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MessageRepository for SeaOrmMessageRepository {
    async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>> {
        let model = message::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn create_message(&self, new_message: &NewMessage) -> RepoResult<Message> {
        let active = message::ActiveModel {
            message_thread_id: Set(new_message.message_thread_id.into()),
            parent_message_id: Set(new_message.parent_message_id.map(Into::into)),
            sent_by: Set(new_message.sent_by.into()),
            text: Set(new_message.text.to_owned()),
            is_deleted: Set(false),
            has_attachments: Set(!new_message.attachments.is_empty()),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn list_messages(
        &self,
        thread_id: MessageThreadId,
        limit: u32,
        dir: PageDir,
        cursor: Option<MessageCursor>,
        include_deleted: bool,
        order: SortOrder,
    ) -> RepoResult<Page<Message, MessageCursor>> {
        let mut q = message::Entity::find()
            .filter(message::Column::MessageThreadId.eq::<Uuid>(thread_id.into()));

        if !include_deleted {
            q = q.filter(message::Column::IsDeleted.eq(false));
        }

        if let Some(cursor) = cursor {
            let cursor_id: Uuid = cursor.message_id.into();
            let cursor_ts: DateTimeWithTimeZone = cursor.sent_at.into();

            let cond = match dir {
                PageDir::Before => Condition::any()
                    .add(message::Column::SentAt.lt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(message::Column::SentAt.eq(cursor_ts))
                            .add(message::Column::Id.lt(cursor_id)),
                    ),
                PageDir::After => Condition::any()
                    .add(message::Column::SentAt.gt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(message::Column::SentAt.eq(cursor_ts))
                            .add(message::Column::Id.gt(cursor_id)),
                    ),
            };

            q = q.filter(cond);
        }

        q = match order {
            SortOrder::Asc => q
                .order_by_asc(message::Column::SentAt)
                .order_by_asc(message::Column::Id),
            SortOrder::Desc => q
                .order_by_desc(message::Column::SentAt)
                .order_by_desc(message::Column::Id),
        };

        let items = q
            .limit(limit as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<Message>>();

        let (min, max) = match order {
            SortOrder::Asc => (items.first(), items.last()),
            SortOrder::Desc => (items.last(), items.first()),
        };

        let min = min.map(|m| MessageCursor {
            sent_at: m.sent_at,
            message_id: m.id,
        });
        let max = max.map(|m| MessageCursor {
            sent_at: m.sent_at,
            message_id: m.id,
        });

        let (next, prev) = match dir {
            PageDir::After => (max, min),
            PageDir::Before => (min, max),
        };

        Ok(Page::new(items, next, prev))
    }

    async fn update_message_text(
        &self,
        message_id: MessageId,
        new_text: String,
    ) -> RepoResult<Message> {
        let msg = message::Entity::find_by_id(message_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: message::ActiveModel = msg.into();
        active.text = Set(Some(new_text));
        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn soft_delete_message(&self, message_id: MessageId) -> RepoResult<()> {
        let msg = message::Entity::find_by_id(message_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        let Some(msg) = msg else {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        };

        let mut active: message::ActiveModel = msg.into();
        active.is_deleted = Set(true);
        active.update(&self.db).await.map_err(map_db_err)?;

        Ok(())
    }
}
