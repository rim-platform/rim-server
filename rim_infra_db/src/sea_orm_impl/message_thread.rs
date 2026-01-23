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
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DbConn, EntityTrait, QueryFilter, QueryOrder, Set,
};

use crate::error::map_db_err;
use rim_core::repository::error::{RepoError, RepoErrorKind};

pub struct SeaOrmMessageThreadRepository {
    pub db: DbConn,
}

impl SeaOrmMessageThreadRepository {
    pub fn new(db: DbConn) -> Self {
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

    async fn create_thread(&self, conversation_id: ConversationId) -> RepoResult<MessageThread> {
        let now: DateTimeWithTimeZone = chrono::Utc::now().into();
        let active = message_thread::ActiveModel {
            conversation_id: Set(conversation_id.into()),
            first_message_id: Set(None),
            created_at: Set(now.into()),
            last_message_at: Set(now.into()),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn set_first_message(
        &self,
        thread_id: MessageThreadId,
        message_id: MessageId,
    ) -> RepoResult<()> {
        let thread = message_thread::Entity::find_by_id(thread_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        let Some(thread) = thread else {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        };

        let mut active: message_thread::ActiveModel = thread.into();
        active.first_message_id = Set(Some(message_id.into()));
        active.update(&self.db).await.map_err(map_db_err)?;

        Ok(())
    }

    async fn touch_last_message_at(
        &self,
        thread_id: MessageThreadId,
        at: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<()> {
        let at: DateTimeWithTimeZone = at.into();
        let thread = message_thread::Entity::find_by_id(thread_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        let Some(thread) = thread else {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        };

        let mut active: message_thread::ActiveModel = thread.into();
        active.last_message_at = Set(at);
        active.update(&self.db).await.map_err(map_db_err)?;

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

        if let Some(cursor) = cursor {
            let cursor_id: Uuid = cursor.thread_id.into();
            let cursor_ts: DateTimeWithTimeZone = cursor.last_message_at.into();

            let cond = match dir {
                PageDir::Before => Condition::any()
                    .add(message_thread::Column::LastMessageAt.lt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(message_thread::Column::LastMessageAt.eq(cursor_ts))
                            .add(message_thread::Column::Id.lt(cursor_id)),
                    ),
                PageDir::After => Condition::any()
                    .add(message_thread::Column::LastMessageAt.gt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(message_thread::Column::LastMessageAt.eq(cursor_ts))
                            .add(message_thread::Column::Id.gt(cursor_id)),
                    ),
            };

            q = q.filter(cond);
        }

        q = q
            .order_by_desc(message_thread::Column::LastMessageAt)
            .order_by_desc(message_thread::Column::Id);

        let items = q
            .limit(limit as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<MessageThread>>();

        // Fixed ordering: `last_message_at DESC, id DESC`.
        let min = items.last().map(|t| ThreadCursor {
            last_message_at: t.last_message_at,
            thread_id: t.id,
        });
        let max = items.first().map(|t| ThreadCursor {
            last_message_at: t.last_message_at,
            thread_id: t.id,
        });

        let (next, prev) = match dir {
            PageDir::After => (max, min),
            PageDir::Before => (min, max),
        };

        Ok(Page::new(items, next, prev))
    }
}
