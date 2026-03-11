use async_trait::async_trait;
use entity::{conversation, conversation_member};
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
    QueryOrder, QuerySelect, QueryTrait, RelationTrait, Set, TransactionTrait,
};

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
        title: Option<String>,
    ) -> RepoResult<Conversation> {
        let active = conversation::ActiveModel {
            id: Set(ConversationId::new().into()),
            conversation_type: Set(conversation_type.into()),
            server_id: Set(server_id.into()),
            title: Set(title),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn create_conversation_with_members(
        &self,
        conversation_type: ConversationType,
        server_id: ServerId,
        title: Option<String>,
        member_ids: &[UserId],
    ) -> RepoResult<Conversation> {
        let server_id_uuid: Uuid = server_id.into();
        let member_ids = member_ids
            .iter()
            .copied()
            .map(Into::into)
            .collect::<Vec<Uuid>>();

        let inserted = self
            .db
            .transaction::<_, conversation::Model, RepoError>(|txn| {
                Box::pin(async move {
                    let active = conversation::ActiveModel {
                        id: Set(ConversationId::new().into()),
                        conversation_type: Set(conversation_type.into()),
                        server_id: Set(server_id_uuid),
                        title: Set(title.clone()),
                        ..Default::default()
                    };

                    let inserted = active.insert(txn).await.map_err(map_db_err)?;

                    for user_id in member_ids {
                        let member = conversation_member::ActiveModel {
                            id: Set(
                                rim_core::conversation_member::model::ConversationMemberId::new()
                                    .into(),
                            ),
                            conversation_id: Set(inserted.id),
                            user_id: Set(user_id),
                        };
                        member.insert(txn).await.map_err(map_db_err)?;
                    }

                    Ok(inserted)
                })
            })
            .await
            .map_err(|e| match e {
                sea_orm::TransactionError::Connection(db) => map_db_err(db),
                sea_orm::TransactionError::Transaction(err) => err,
            })?;

        Ok(inserted.into())
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

        let has_cursor = cursor.is_some();
        if let Some(cursor) = cursor {
            let cursor_id: Uuid = cursor.conversation_id.into();
            let cursor_ts: DateTimeWithTimeZone = cursor.last_activity_at.into();

            let cond = match dir {
                PageDir::Before => Condition::any()
                    .add(conversation::Column::LastActivityAt.gt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(conversation::Column::LastActivityAt.eq(cursor_ts))
                            .add(conversation::Column::Id.gt(cursor_id)),
                    ),
                PageDir::After => Condition::any()
                    .add(conversation::Column::LastActivityAt.lt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(conversation::Column::LastActivityAt.eq(cursor_ts))
                            .add(conversation::Column::Id.lt(cursor_id)),
                    ),
            };
            q = q.filter(cond);
        }

        q = q
            .order_by_desc(conversation::Column::LastActivityAt)
            .order_by_desc(conversation::Column::Id);

        let mut items = q
            .limit((limit + 1) as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<Conversation>>();

        let has_more_in_dir = items.len() > limit as usize;
        if has_more_in_dir {
            items.truncate(limit as usize);
        }

        // Fixed ordering: `last_activity_at DESC, id DESC`.
        let min = items.last().map(|c| ConversationActivityCursor {
            last_activity_at: c.last_activity_at,
            conversation_id: c.id,
        });
        let max = items.first().map(|c| ConversationActivityCursor {
            last_activity_at: c.last_activity_at,
            conversation_id: c.id,
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

        let has_cursor = cursor.is_some();
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

        let mut items = q
            .limit((limit + 1) as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<Conversation>>();

        let has_more_in_dir = items.len() > limit as usize;
        if has_more_in_dir {
            items.truncate(limit as usize);
        }

        // Fixed ordering: `created_at ASC, id ASC`.
        let min = items.first().map(|c| ConversationCreatedCursor {
            created_at: c.created_at,
            conversation_id: c.id,
        });
        let max = items.last().map(|c| ConversationCreatedCursor {
            created_at: c.created_at,
            conversation_id: c.id,
        });

        let has_opposite = has_cursor && !items.is_empty();
        let (next, prev) = match dir {
            PageDir::After => (
                has_more_in_dir.then_some(max).flatten(),
                has_opposite.then_some(min).flatten(),
            ),
            PageDir::Before => (
                has_more_in_dir.then_some(min).flatten(),
                has_opposite.then_some(max).flatten(),
            ),
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
