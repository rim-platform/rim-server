use async_trait::async_trait;
use entity::message;
use rim_core::conversation::model::ConversationId;
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
use std::collections::HashMap;

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

    async fn get_by_ids(&self, ids: &[MessageId]) -> RepoResult<HashMap<MessageId, Message>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }

        let ids = ids.iter().copied().map(Into::into).collect::<Vec<Uuid>>();
        let models = message::Entity::find()
            .filter(message::Column::Id.is_in(ids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models
            .into_iter()
            .map(Into::<Message>::into)
            .map(|m| (m.id, m))
            .collect())
    }

    async fn create_message(&self, new_message: &NewMessage) -> RepoResult<Message> {
        let active = message::ActiveModel {
            id: Set(MessageId::new().into()),
            conversation_id: Set(new_message.conversation_id.into()),
            thread_id: Set(new_message.thread_id.map(Into::into)),
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
        conversation_id: ConversationId,
        thread_id: Option<MessageThreadId>,
        limit: u32,
        dir: PageDir,
        cursor: Option<MessageCursor>,
        include_deleted: bool,
        order: SortOrder,
    ) -> RepoResult<Page<Message, MessageCursor>> {
        let mut q = message::Entity::find()
            .filter(message::Column::ConversationId.eq::<Uuid>(conversation_id.into()));

        q = match thread_id {
            Some(thread_id) => q.filter(message::Column::ThreadId.eq::<Uuid>(thread_id.into())),
            None => q.filter(message::Column::ThreadId.is_null()),
        };

        if !include_deleted {
            q = q.filter(message::Column::IsDeleted.eq(false));
        }

        let has_cursor = cursor.is_some();
        if let Some(cursor) = cursor {
            let cursor_id: Uuid = cursor.message_id.into();
            let cursor_ts: DateTimeWithTimeZone = cursor.sent_at.into();

            let cond = match (order, dir) {
                (SortOrder::Asc, PageDir::Before) | (SortOrder::Desc, PageDir::After) => {
                    Condition::any()
                        .add(message::Column::SentAt.lt(cursor_ts))
                        .add(
                            Condition::all()
                                .add(message::Column::SentAt.eq(cursor_ts))
                                .add(message::Column::Id.lt(cursor_id)),
                        )
                }
                (SortOrder::Asc, PageDir::After) | (SortOrder::Desc, PageDir::Before) => {
                    Condition::any()
                        .add(message::Column::SentAt.gt(cursor_ts))
                        .add(
                            Condition::all()
                                .add(message::Column::SentAt.eq(cursor_ts))
                                .add(message::Column::Id.gt(cursor_id)),
                        )
                }
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

        let mut items = q
            .limit((limit + 1) as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<Message>>();

        let has_more_in_dir = items.len() > limit as usize;
        if has_more_in_dir {
            items.truncate(limit as usize);
        }

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

        let has_opposite = has_cursor && !items.is_empty();
        let (next, prev) = match (order, dir) {
            (SortOrder::Asc, PageDir::After) => (
                has_more_in_dir.then_some(max).flatten(),
                has_opposite.then_some(min).flatten(),
            ),
            (SortOrder::Asc, PageDir::Before) => (
                has_more_in_dir.then_some(min).flatten(),
                has_opposite.then_some(max).flatten(),
            ),
            (SortOrder::Desc, PageDir::After) => (
                has_more_in_dir.then_some(min).flatten(),
                has_opposite.then_some(max).flatten(),
            ),
            (SortOrder::Desc, PageDir::Before) => (
                has_more_in_dir.then_some(max).flatten(),
                has_opposite.then_some(min).flatten(),
            ),
        };

        Ok(Page::new(items, next, prev))
    }

    async fn search_messages(
        &self,
        conversation_id: ConversationId,
        query: &str,
        limit: u32,
        include_deleted: bool,
    ) -> RepoResult<Vec<Message>> {
        let mut q = message::Entity::find()
            .filter(message::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .filter(message::Column::Text.contains(query));

        if !include_deleted {
            q = q.filter(message::Column::IsDeleted.eq(false));
        }

        let items = q
            .order_by_desc(message::Column::SentAt)
            .order_by_desc(message::Column::Id)
            .limit(limit as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(Into::into)
            .collect();

        Ok(items)
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

    async fn set_has_attachments(
        &self,
        message_id: MessageId,
        has_attachments: bool,
    ) -> RepoResult<Message> {
        let msg = message::Entity::find_by_id(message_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: message::ActiveModel = msg.into();
        active.has_attachments = Set(has_attachments);
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
