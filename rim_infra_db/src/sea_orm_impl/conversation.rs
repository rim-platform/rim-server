use async_trait::async_trait;
use entity::{conversation, conversation_member, message_thread};
use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
use rim_core::conversation::repository::{
    ConversationActivityCursor, ConversationCreatedCursor, ConversationRepository,
};
use rim_core::repository::error::RepoResult;
use rim_core::repository::error::{RepoError, RepoErrorKind};
use rim_core::repository::pagination::{Page, PageDir};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DbConn, EntityTrait, JoinType, QueryFilter,
    QueryOrder, QuerySelect, QueryTrait, RelationTrait, Set,
};
use sea_orm::TransactionTrait;

use crate::error::map_db_err;

pub struct SeaOrmConversationRepository {
    pub db: DbConn,
}

impl SeaOrmConversationRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ConversationRepository for SeaOrmConversationRepository {
    async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>> {
        let model = conversation::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn create_conversation(
        &self,
        conversation_type: ConversationType,
        server_id: ServerId,
    ) -> RepoResult<Conversation> {
        // Insert conversation and its main thread in a single transaction.
        // The conversation holds a NOT NULL FK to `message_thread` via `main_thread_id`.
        let inserted = self
            .db
            .transaction::<_, conversation::Model, RepoError>(|txn| {
                let conversation_id = ConversationId::new();
                let main_thread_id = rim_core::message_thread::model::MessageThreadId::new();
                let now = chrono::Utc::now();

                Box::pin(async move {
                    // Insert conversation first so the thread FK is satisfied.
                    let active = conversation::ActiveModel {
                        id: Set(conversation_id.into()),
                        conversation_type: Set(conversation_type.into()),
                        server_id: Set(server_id.into()),
                        main_thread_id: Set(main_thread_id.into()),
                        ..Default::default()
                    };
                    active.insert(txn).await.map_err(map_db_err)?;

                    let thread_active = message_thread::ActiveModel {
                        id: Set(main_thread_id.into()),
                        conversation_id: Set(conversation_id.into()),
                        first_message_id: Set(None),
                        created_at: Set(now.into()),
                        last_message_at: Set(now.into()),
                    };
                    thread_active.insert(txn).await.map_err(map_db_err)?;

                    // Return the conversation.
                    let inserted = conversation::Entity::find_by_id(conversation_id)
                        .one(txn)
                        .await
                        .map_err(map_db_err)?
                        .expect("conversation must exist after insert");
                    Ok(inserted)
                })
            })
            .await
            .map_err(|e| match e {
                sea_orm::TransactionError::Connection(db) => map_db_err(db),
                sea_orm::TransactionError::Transaction(err) => err,
            })?;

        return Ok(inserted.into());

        // unreachable: returned above
    }

    async fn find_direct_between_users(
        &self,
        server_id: ServerId,
        user_a: UserId,
        user_b: UserId,
    ) -> RepoResult<Option<Conversation>> {
        let direct: entity::sea_orm_active_enums::ConversationType =
            ConversationType::Direct.into();

        let user_b_subquery = conversation_member::Entity::find()
            .select_only()
            .column(conversation_member::Column::ConversationId)
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_b.into()))
            .into_query();

        let model = conversation::Entity::find()
            .filter(conversation::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(conversation::Column::ConversationType.eq(direct))
            .filter(conversation::Column::Id.in_subquery(user_b_subquery))
            .join(
                JoinType::InnerJoin,
                conversation::Relation::ConversationMember.def(),
            )
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_a.into()))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn list_inbox_conversations_for_user(
        &self,
        user_id: UserId,
        server_id: ServerId,
        limit: u32,
        dir: PageDir,
        cursor: Option<ConversationActivityCursor>,
    ) -> RepoResult<Page<Conversation, ConversationActivityCursor>> {
        let mut q = conversation::Entity::find()
            .join(
                JoinType::InnerJoin,
                conversation::Relation::ConversationMember.def(),
            )
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(conversation::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(
                conversation::Column::ConversationType
                    .is_in([
                        entity::sea_orm_active_enums::ConversationType::Direct,
                        entity::sea_orm_active_enums::ConversationType::Group,
                    ])
                    .to_owned(),
            );

        if let Some(cursor) = cursor {
            let cursor_id: Uuid = cursor.conversation_id.into();
            let cursor_ts: DateTimeWithTimeZone = cursor.last_activity_at.into();

            let cond = match dir {
                PageDir::Before => Condition::any()
                    .add(conversation::Column::LastActivityAt.lt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(conversation::Column::LastActivityAt.eq(cursor_ts))
                            .add(conversation::Column::Id.lt(cursor_id)),
                    ),
                PageDir::After => Condition::any()
                    .add(conversation::Column::LastActivityAt.gt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(conversation::Column::LastActivityAt.eq(cursor_ts))
                            .add(conversation::Column::Id.gt(cursor_id)),
                    ),
            };
            q = q.filter(cond);
        }

        q = q
            .order_by_desc(conversation::Column::LastActivityAt)
            .order_by_desc(conversation::Column::Id);

        let items = q
            .limit(limit as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<Conversation>>();

        // Fixed ordering: `last_activity_at DESC, id DESC`.
        let min = items.last().map(|c| ConversationActivityCursor {
            last_activity_at: c.last_activity_at,
            conversation_id: c.id,
        });
        let max = items.first().map(|c| ConversationActivityCursor {
            last_activity_at: c.last_activity_at,
            conversation_id: c.id,
        });

        let (next, prev) = match dir {
            PageDir::After => (max, min),
            PageDir::Before => (min, max),
        };

        Ok(Page::new(items, next, prev))
    }

    async fn list_channels_for_user(
        &self,
        user_id: UserId,
        server_id: ServerId,
        limit: u32,
        dir: PageDir,
        cursor: Option<ConversationCreatedCursor>,
    ) -> RepoResult<Page<Conversation, ConversationCreatedCursor>> {
        let mut q = conversation::Entity::find()
            .join(
                JoinType::InnerJoin,
                conversation::Relation::ConversationMember.def(),
            )
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(conversation::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(
                conversation::Column::ConversationType
                    .eq(entity::sea_orm_active_enums::ConversationType::Channel)
                    .to_owned(),
            );

        if let Some(cursor) = cursor {
            let cursor_id: Uuid = cursor.conversation_id.into();
            let cursor_ts: DateTimeWithTimeZone = cursor.created_at.into();

            let cond = match dir {
                PageDir::Before => Condition::any()
                    .add(conversation::Column::CreatedAt.lt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(conversation::Column::CreatedAt.eq(cursor_ts))
                            .add(conversation::Column::Id.lt(cursor_id)),
                    ),
                PageDir::After => Condition::any()
                    .add(conversation::Column::CreatedAt.gt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(conversation::Column::CreatedAt.eq(cursor_ts))
                            .add(conversation::Column::Id.gt(cursor_id)),
                    ),
            };
            q = q.filter(cond);
        }

        // Channels are stable: oldest first.
        q = q
            .order_by_asc(conversation::Column::CreatedAt)
            .order_by_asc(conversation::Column::Id);

        let items = q
            .limit(limit as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<Conversation>>();

        // Fixed ordering: `created_at ASC, id ASC`.
        let min = items.first().map(|c| ConversationCreatedCursor {
            created_at: c.created_at,
            conversation_id: c.id,
        });
        let max = items.last().map(|c| ConversationCreatedCursor {
            created_at: c.created_at,
            conversation_id: c.id,
        });

        let (next, prev) = match dir {
            PageDir::After => (max, min),
            PageDir::Before => (min, max),
        };

        Ok(Page::new(items, next, prev))
    }

    async fn touch_last_activity_at(
        &self,
        conversation_id: ConversationId,
        at: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<()> {
        let model = conversation::Entity::find_by_id(conversation_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        let Some(model) = model else {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        };

        let mut active: conversation::ActiveModel = model.into();
        let at: DateTimeWithTimeZone = at.into();
        active.last_activity_at = Set(at);
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}
