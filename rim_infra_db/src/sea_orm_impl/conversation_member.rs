use async_trait::async_trait;
use entity::conversation_member;
use rim_core::conversation::model::ConversationId;
use rim_core::conversation_member::model::{ConversationMember, ConversationMemberId, Membership};
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;
use sea_orm::ColumnTrait;
use sea_orm::prelude::{DateTimeWithTimeZone, Expr, Uuid};
use sea_orm::{ActiveModelTrait, EntityTrait, QueryFilter, QuerySelect, Set};

use crate::DbHandle;
use std::collections::HashMap;

use crate::error::map_db_err;
use rim_core::repository::error::{RepoError, RepoErrorKind};

pub struct SeaOrmConversationMemberRepository {
    db: DbHandle,
}

impl SeaOrmConversationMemberRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    async fn insert(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
        pending_since: Option<DateTimeWithTimeZone>,
    ) -> RepoResult<ConversationMember> {
        let active = conversation_member::ActiveModel {
            id: Set(ConversationMemberId::new().into()),
            conversation_id: Set(conversation_id.into()),
            user_id: Set(user_id.into()),
            pending_since: Set(pending_since),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    /// The members of each conversation, active or pending as `pending` says.
    async fn member_ids_by_conversation(
        &self,
        conversation_ids: &[ConversationId],
        pending: bool,
    ) -> RepoResult<HashMap<ConversationId, Vec<UserId>>> {
        if conversation_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let ids = conversation_ids
            .iter()
            .copied()
            .map(Into::into)
            .collect::<Vec<Uuid>>();
        let rows: Vec<(Uuid, Uuid)> = conversation_member::Entity::find()
            .select_only()
            .column(conversation_member::Column::ConversationId)
            .column(conversation_member::Column::UserId)
            .filter(conversation_member::Column::ConversationId.is_in(ids))
            .filter(pending_filter(pending))
            .into_tuple()
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        let mut out = HashMap::<ConversationId, Vec<UserId>>::new();
        for (conversation_id, user_id) in rows {
            out.entry(conversation_id.into())
                .or_default()
                .push(user_id.into());
        }
        Ok(out)
    }
}

/// Rows that are pending, or active when `pending` is false.
fn pending_filter(pending: bool) -> sea_orm::Condition {
    let column = conversation_member::Column::PendingSince;
    sea_orm::Condition::all().add(if pending {
        column.is_not_null()
    } else {
        column.is_null()
    })
}

#[async_trait]
impl ConversationMemberRepository for SeaOrmConversationMemberRepository {
    async fn add_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<ConversationMember> {
        self.insert(conversation_id, user_id, None).await
    }

    async fn add_pending_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<ConversationMember> {
        self.insert(conversation_id, user_id, Some(chrono::Utc::now().into()))
            .await
    }

    async fn remove_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<()> {
        let res = conversation_member::Entity::delete_many()
            .filter(conversation_member::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        if res.rows_affected == 0 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }

        Ok(())
    }

    async fn is_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<bool> {
        Ok(self.membership(conversation_id, user_id).await? == Some(Membership::Active))
    }

    async fn membership(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<Option<Membership>> {
        let member = conversation_member::Entity::find()
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(conversation_member::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(member.map(|m| match m.pending_since {
            Some(_) => Membership::Pending,
            None => Membership::Active,
        }))
    }

    async fn list_member_ids(&self, conversation_id: ConversationId) -> RepoResult<Vec<UserId>> {
        Ok(self
            .member_ids_by_conversation(&[conversation_id], false)
            .await?
            .remove(&conversation_id)
            .unwrap_or_default())
    }

    async fn list_member_ids_for_conversations(
        &self,
        conversation_ids: &[ConversationId],
    ) -> RepoResult<HashMap<ConversationId, Vec<UserId>>> {
        self.member_ids_by_conversation(conversation_ids, false)
            .await
    }

    async fn list_pending_member_ids(
        &self,
        conversation_id: ConversationId,
    ) -> RepoResult<Vec<UserId>> {
        Ok(self
            .member_ids_by_conversation(&[conversation_id], true)
            .await?
            .remove(&conversation_id)
            .unwrap_or_default())
    }

    async fn list_pending_member_ids_for_conversations(
        &self,
        conversation_ids: &[ConversationId],
    ) -> RepoResult<HashMap<ConversationId, Vec<UserId>>> {
        self.member_ids_by_conversation(conversation_ids, true)
            .await
    }

    async fn list_pending_conversation_ids(
        &self,
        user_id: UserId,
    ) -> RepoResult<Vec<ConversationId>> {
        let ids: Vec<Uuid> = conversation_member::Entity::find()
            .select_only()
            .column(conversation_member::Column::ConversationId)
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(pending_filter(true))
            .into_tuple()
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(ids.into_iter().map(Into::into).collect())
    }

    async fn delete_pending_before(
        &self,
        cutoff: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<u64> {
        let cutoff: DateTimeWithTimeZone = cutoff.into();
        let res = conversation_member::Entity::delete_many()
            .filter(conversation_member::Column::PendingSince.lt(cutoff))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }

    async fn activate_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<bool> {
        // `joined_at` is the unread baseline; a member reads nothing from before activation.
        let now: DateTimeWithTimeZone = chrono::Utc::now().into();
        let res = conversation_member::Entity::update_many()
            .col_expr(
                conversation_member::Column::PendingSince,
                Expr::value(Option::<DateTimeWithTimeZone>::None),
            )
            .col_expr(conversation_member::Column::JoinedAt, Expr::value(now))
            .filter(conversation_member::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(pending_filter(true))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected > 0)
    }
}
