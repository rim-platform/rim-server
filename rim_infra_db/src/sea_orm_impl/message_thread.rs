use async_trait::async_trait;
use entity::message_thread;
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message_thread::model::{MessageThread, MessageThreadId};
use rim_core::message_thread::repository::{MessageThreadRepository, ThreadCursor};
use rim_core::repository::error::RepoResult;
use rim_core::repository::pagination::{Page, PageDir};
use sea_orm::QuerySelect;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::prelude::Expr;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, Set,
};
use std::collections::HashMap;

use crate::DbHandle;
use crate::error::map_db_err;
use rim_core::repository::error::{RepoError, RepoErrorKind};

pub struct SeaOrmMessageThreadRepository {
    pub db: DbHandle,
}

impl SeaOrmMessageThreadRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MessageThreadRepository for SeaOrmMessageThreadRepository {
    async fn get_by_id(&self, id: MessageThreadId) -> RepoResult<Option<MessageThread>> {
        let model = message_thread::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn get_by_ids(&self, ids: &[MessageThreadId]) -> RepoResult<Vec<MessageThread>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let uuids: Vec<Uuid> = ids.iter().copied().map(Into::into).collect();
        let models = message_thread::Entity::find()
            .filter(message_thread::Column::Id.is_in(uuids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn get_by_root_message(
        &self,
        message_id: MessageId,
    ) -> RepoResult<Option<MessageThread>> {
        let model = message_thread::Entity::find()
            .filter(message_thread::Column::FirstMessageId.eq::<Uuid>(message_id.into()))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn get_thread_ids_by_root_messages(
        &self,
        message_ids: &[MessageId],
    ) -> RepoResult<HashMap<MessageId, MessageThreadId>> {
        if message_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let ids = message_ids
            .iter()
            .copied()
            .map(Into::into)
            .collect::<Vec<Uuid>>();
        let models = message_thread::Entity::find()
            .filter(message_thread::Column::FirstMessageId.is_in(ids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models
            .into_iter()
            .filter_map(|model| {
                model
                    .first_message_id
                    .map(|root_id| (root_id.into(), model.id.into()))
            })
            .collect())
    }

    async fn create_thread_for_root(
        &self,
        conversation_id: ConversationId,
        message_id: MessageId,
    ) -> RepoResult<MessageThread> {
        let conversation_id_uuid: Uuid = conversation_id.into();
        let message_id_uuid: Uuid = message_id.into();

        let inserted = self
            .db
            .transaction::<_, message_thread::Model, RepoError>(|txn| {
                Box::pin(async move {
                    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
                    let active = message_thread::ActiveModel {
                        id: Set(MessageThreadId::new().into()),
                        conversation_id: Set(conversation_id_uuid),
                        first_message_id: Set(Some(message_id_uuid)),
                        created_at: Set(now),
                        last_message_at: Set(now),
                    };

                    active.insert(txn).await.map_err(map_db_err)
                })
            })
            .await
            .map_err(|e| match e {
                sea_orm::TransactionError::Connection(db) => map_db_err(db),
                sea_orm::TransactionError::Transaction(err) => err,
            })?;

        Ok(inserted.into())
    }

    async fn touch_last_message_at(
        &self,
        thread_id: MessageThreadId,
        at: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<()> {
        let at: DateTimeWithTimeZone = at.into();
        let result = message_thread::Entity::update_many()
            .col_expr(message_thread::Column::LastMessageAt, Expr::value(at))
            .filter(message_thread::Column::Id.eq(Uuid::from(thread_id)))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        if result.rows_affected == 0 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }

        Ok(())
    }

    async fn list_threads_for_conversation(
        &self,
        conversation_id: ConversationId,
        limit: u32,
        dir: PageDir,
        cursor: Option<ThreadCursor>,
    ) -> RepoResult<Page<MessageThread, ThreadCursor>> {
        let mut q = message_thread::Entity::find()
            .filter(message_thread::Column::ConversationId.eq::<Uuid>(conversation_id.into()));

        let has_cursor = cursor.is_some();
        if let Some(cursor) = cursor {
            let cursor_id: Uuid = cursor.thread_id.into();
            let cursor_ts: DateTimeWithTimeZone = cursor.last_message_at.into();

            let cond = match dir {
                PageDir::Before => Condition::any()
                    .add(message_thread::Column::LastMessageAt.gt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(message_thread::Column::LastMessageAt.eq(cursor_ts))
                            .add(message_thread::Column::Id.gt(cursor_id)),
                    ),
                PageDir::After => Condition::any()
                    .add(message_thread::Column::LastMessageAt.lt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(message_thread::Column::LastMessageAt.eq(cursor_ts))
                            .add(message_thread::Column::Id.lt(cursor_id)),
                    ),
            };

            q = q.filter(cond);
        }

        // A page holds the rows adjacent to the cursor. `Before` rows precede the cursor in the
        // endpoint order, so fetch them nearest-first (reversed order) and flip the page back
        // below; fetching in endpoint order would return the far end of the range and skip
        // everything between it and the cursor (finding A-H1).
        let fetch_reversed = has_cursor && dir == PageDir::Before;
        q = if fetch_reversed {
            q.order_by_asc(message_thread::Column::LastMessageAt)
                .order_by_asc(message_thread::Column::Id)
        } else {
            q.order_by_desc(message_thread::Column::LastMessageAt)
                .order_by_desc(message_thread::Column::Id)
        };

        let mut items = q
            .limit((limit + 1) as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<MessageThread>>();

        let has_more_in_dir = items.len() > limit as usize;
        if has_more_in_dir {
            items.truncate(limit as usize);
        }
        if fetch_reversed {
            items.reverse();
        }

        // Fixed ordering: `last_message_at DESC, id DESC`.
        let min = items.last().map(|t| ThreadCursor {
            last_message_at: t.last_message_at,
            thread_id: t.id,
        });
        let max = items.first().map(|t| ThreadCursor {
            last_message_at: t.last_message_at,
            thread_id: t.id,
        });

        let has_opposite = has_cursor && !items.is_empty();
        let (next, prev) = match dir {
            PageDir::After => (
                has_more_in_dir.then_some(min).flatten(),
                has_opposite.then_some(max).flatten(),
            ),
            PageDir::Before => (
                has_more_in_dir.then_some(max).flatten(),
                has_opposite.then_some(min).flatten(),
            ),
        };

        Ok(Page::new(items, next, prev))
    }
}
