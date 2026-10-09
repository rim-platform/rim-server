use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::{conversation, conversation_member};
use rim_core::conversation::model::{
    Conversation, ConversationId, ConversationType, NewConversation,
};
use rim_core::conversation::repository::{
    ChannelListFilter, ConversationActivityCursor, ConversationCreatedCursor,
    ConversationRepository,
};
use rim_core::repository::error::RepoResult;
use rim_core::repository::error::{RepoError, RepoErrorKind};
use rim_core::repository::pagination::{Page, PageDir};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::prelude::Expr;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, EntityTrait, JoinType, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, QueryTrait, RelationTrait, Set,
};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmConversationRepository {
    pub db: DbHandle,
}

impl SeaOrmConversationRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    fn new_active_model(conv: NewConversation) -> conversation::ActiveModel {
        let mut active = conversation::ActiveModel {
            id: Set(ConversationId::new().into()),
            conversation_type: Set(conv.conversation_type.into()),
            server_id: Set(conv.server_id.into()),
            title: Set(conv.title),
            is_private: Set(conv.is_private),
            created_by: Set(conv.created_by.map(Into::into)),
            ..Default::default()
        };
        // An encrypted conversation is encrypted from the instant it is created.
        if conv.encrypted {
            let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
            active.created_at = Set(now);
            active.encrypted_at = Set(Some(now));
        }
        active
    }

    /// `SELECT conversation_id FROM conversation_member WHERE user_id = $user`.
    fn member_conversation_ids(user_id: UserId) -> sea_orm::sea_query::SelectStatement {
        conversation_member::Entity::find()
            .select_only()
            .column(conversation_member::Column::ConversationId)
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .into_query()
    }

    /// Rows `user_id` may read: public channels, or any conversation they are a member of.
    fn visible_to(user_id: UserId) -> Condition {
        Condition::any()
            .add(
                Condition::all()
                    .add(
                        conversation::Column::ConversationType
                            .eq(entity::sea_orm_active_enums::ConversationType::Channel),
                    )
                    .add(conversation::Column::IsPrivate.eq(false)),
            )
            .add(conversation::Column::Id.in_subquery(Self::member_conversation_ids(user_id)))
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

    async fn get_by_ids(&self, ids: &[ConversationId]) -> RepoResult<Vec<Conversation>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let uuids: Vec<Uuid> = ids.iter().copied().map(Into::into).collect();
        let models = conversation::Entity::find()
            .filter(conversation::Column::Id.is_in(uuids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn create_conversation(&self, conv: NewConversation) -> RepoResult<Conversation> {
        let active = Self::new_active_model(conv);

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn create_conversation_with_members(
        &self,
        conv: NewConversation,
        member_ids: &[UserId],
    ) -> RepoResult<Conversation> {
        let member_ids = member_ids
            .iter()
            .copied()
            .map(Into::into)
            .collect::<Vec<Uuid>>();

        let inserted = self
            .db
            .transaction::<_, conversation::Model, RepoError>(|txn| {
                Box::pin(async move {
                    let active = Self::new_active_model(conv);

                    let inserted = active.insert(txn).await.map_err(map_db_err)?;

                    for user_id in member_ids {
                        let member = conversation_member::ActiveModel {
                            id: Set(
                                rim_core::conversation_member::model::ConversationMemberId::new()
                                    .into(),
                            ),
                            conversation_id: Set(inserted.id),
                            user_id: Set(user_id),
                            ..Default::default()
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

        // A page holds the rows adjacent to the cursor. `Before` rows precede the cursor in the
        // endpoint order, so fetch them nearest-first (reversed order) and flip the page back
        // below; fetching in endpoint order would return the far end of the range and skip
        // everything between it and the cursor (finding A-H1).
        let fetch_reversed = has_cursor && dir == PageDir::Before;
        q = if fetch_reversed {
            q.order_by_asc(conversation::Column::LastActivityAt)
                .order_by_asc(conversation::Column::Id)
        } else {
            q.order_by_desc(conversation::Column::LastActivityAt)
                .order_by_desc(conversation::Column::Id)
        };

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
        if fetch_reversed {
            items.reverse();
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
        filter: ChannelListFilter,
        limit: u32,
        dir: PageDir,
        cursor: Option<ConversationCreatedCursor>,
    ) -> RepoResult<Page<Conversation, ConversationCreatedCursor>> {
        // Public channels are visible to every server member; private channels only to their
        // members. Server membership is verified at the service layer.
        let mut q = conversation::Entity::find()
            .filter(conversation::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(
                conversation::Column::ConversationType
                    .eq(entity::sea_orm_active_enums::ConversationType::Channel)
                    .to_owned(),
            )
            .filter(Self::visible_to(user_id));
        if filter.joined_only {
            q = q.filter(
                conversation::Column::Id.in_subquery(Self::member_conversation_ids(user_id)),
            );
        }
        if !filter.include_archived {
            q = q.filter(conversation::Column::ArchivedAt.is_null());
        }

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
        // A page holds the rows adjacent to the cursor. `Before` rows precede the cursor in the
        // endpoint order, so fetch them nearest-first (reversed order) and flip the page back
        // below; fetching in endpoint order would return the far end of the range and skip
        // everything between it and the cursor (finding A-H1).
        let fetch_reversed = has_cursor && dir == PageDir::Before;
        q = if fetch_reversed {
            q.order_by_desc(conversation::Column::CreatedAt)
                .order_by_desc(conversation::Column::Id)
        } else {
            q.order_by_asc(conversation::Column::CreatedAt)
                .order_by_asc(conversation::Column::Id)
        };

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
        if fetch_reversed {
            items.reverse();
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

    async fn exists_channel_by_title(&self, server_id: ServerId, title: &str) -> RepoResult<bool> {
        let channel: entity::sea_orm_active_enums::ConversationType =
            ConversationType::Channel.into();

        let count = conversation::Entity::find()
            .filter(conversation::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(conversation::Column::Title.eq(title))
            .filter(conversation::Column::ConversationType.eq(channel))
            .count(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(count > 0)
    }

    async fn get_channel_by_title(
        &self,
        server_id: ServerId,
        title: &str,
    ) -> RepoResult<Option<Conversation>> {
        let channel: entity::sea_orm_active_enums::ConversationType =
            ConversationType::Channel.into();

        let model = conversation::Entity::find()
            .filter(conversation::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(conversation::Column::Title.eq(title))
            .filter(conversation::Column::ConversationType.eq(channel))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn list_accessible_conversation_ids(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> RepoResult<Vec<ConversationId>> {
        let server_uuid: Uuid = server_id.into();

        // Public channels (archived or not): server membership suffices.
        // Private channels, DMs and groups: only where the user is a conversation_member.
        let models = conversation::Entity::find()
            .filter(conversation::Column::ServerId.eq(server_uuid))
            .filter(Self::visible_to(user_id))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(|m| m.id.into()).collect())
    }

    async fn list_public_channel_ids(
        &self,
        server_id: ServerId,
    ) -> RepoResult<Vec<ConversationId>> {
        let ids: Vec<Uuid> = conversation::Entity::find()
            .select_only()
            .column(conversation::Column::Id)
            .filter(conversation::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(
                conversation::Column::ConversationType
                    .eq(entity::sea_orm_active_enums::ConversationType::Channel),
            )
            .filter(conversation::Column::IsPrivate.eq(false))
            .filter(conversation::Column::ArchivedAt.is_null())
            .into_tuple()
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(ids.into_iter().map(Into::into).collect())
    }

    async fn list_joined_conversation_ids(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> RepoResult<Vec<ConversationId>> {
        let ids: Vec<Uuid> = conversation::Entity::find()
            .select_only()
            .column(conversation::Column::Id)
            .filter(conversation::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(conversation::Column::Id.in_subquery(Self::member_conversation_ids(user_id)))
            .into_tuple()
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(ids.into_iter().map(Into::into).collect())
    }

    async fn touch_last_activity_at(
        &self,
        conversation_id: ConversationId,
        at: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<()> {
        let at: DateTimeWithTimeZone = at.into();
        let result = conversation::Entity::update_many()
            .col_expr(conversation::Column::LastActivityAt, Expr::value(at))
            .filter(conversation::Column::Id.eq(Uuid::from(conversation_id)))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        if result.rows_affected == 0 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }

        Ok(())
    }

    async fn update_topic(
        &self,
        conversation_id: ConversationId,
        topic: Option<String>,
    ) -> RepoResult<Conversation> {
        let model = conversation::Entity::find_by_id(conversation_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: conversation::ActiveModel = model.into();
        active.topic = Set(topic);
        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn set_archived(
        &self,
        conversation_id: ConversationId,
        archived_at: Option<DateTime<Utc>>,
    ) -> RepoResult<Conversation> {
        let model = conversation::Entity::find_by_id(conversation_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: conversation::ActiveModel = model.into();
        active.archived_at = Set(archived_at.map(Into::into));
        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn set_slow_mode(
        &self,
        conversation_id: ConversationId,
        seconds: i32,
    ) -> RepoResult<Conversation> {
        let model = conversation::Entity::find_by_id(conversation_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: conversation::ActiveModel = model.into();
        active.slow_mode_seconds = Set(seconds);
        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }
}
