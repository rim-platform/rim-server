use async_trait::async_trait;
use entity::conversation_member;
use rim_core::conversation::model::ConversationId;
use rim_core::conversation_member::model::{ConversationMember, ConversationMemberId};
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;
use sea_orm::ColumnTrait;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, DbConn, EntityTrait, QueryFilter, Set};

use crate::error::map_db_err;
use rim_core::repository::error::{RepoError, RepoErrorKind};

pub struct SeaOrmConversationMemberRepository {
    db: DbConn,
}

impl SeaOrmConversationMemberRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ConversationMemberRepository for SeaOrmConversationMemberRepository {
    async fn get_by_id(&self, id: ConversationMemberId) -> RepoResult<Option<ConversationMember>> {
        let model = conversation_member::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn add_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<ConversationMember> {
        let active = conversation_member::ActiveModel {
            id: Set(ConversationMemberId::new().into()),
            conversation_id: Set(conversation_id.into()),
            user_id: Set(user_id.into()),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
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

    async fn get_members_for_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> RepoResult<Vec<ConversationMember>> {
        let members = conversation_member::Entity::find()
            .filter(conversation_member::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(members.into_iter().map(Into::into).collect())
    }

    async fn is_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<bool> {
        let member = conversation_member::Entity::find()
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(conversation_member::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(member.is_some())
    }

    async fn list_member_ids(&self, conversation_id: ConversationId) -> RepoResult<Vec<UserId>> {
        let members = conversation_member::Entity::find()
            .filter(conversation_member::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(members.into_iter().map(|m| m.user_id.into()).collect())
    }
}
