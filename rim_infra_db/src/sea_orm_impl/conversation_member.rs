use crate::impl_repository_for_entity;
use async_trait::async_trait;
use entity::conversation_member;
use rim_core::conversation::model::ConversationId;
use rim_core::conversation_member::model::{ConversationMember, ConversationMemberId};
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::repository::repository::Repository;
use rim_core::user::model::UserId;
use sea_orm::ColumnTrait;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, DbConn, EntityTrait, QueryFilter, Set};

pub struct SeaOrmConversationMemberRepository {
    db: DbConn,
}

impl SeaOrmConversationMemberRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

impl_repository_for_entity!(
    SeaOrmConversationMemberRepository,
    conversation_member::Entity,
    ConversationMember,
    ConversationMemberId
);

#[async_trait]
impl ConversationMemberRepository for SeaOrmConversationMemberRepository {
    async fn add_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> anyhow::Result<Self::Model> {
        let active = conversation_member::ActiveModel {
            conversation_id: Set(conversation_id.into()),
            user_id: Set(user_id.into()),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await?;
        Ok(inserted.into())
    }

    async fn remove_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> anyhow::Result<()> {
        conversation_member::Entity::delete_many()
            .filter(conversation_member::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .exec(&self.db)
            .await?;
        Ok(())
    }

    async fn get_members_for_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> anyhow::Result<Vec<Self::Model>> {
        let members = conversation_member::Entity::find()
            .filter(conversation_member::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .all(&self.db)
            .await?;
        Ok(members.into_iter().map(Into::into).collect())
    }

    async fn is_member(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> anyhow::Result<bool> {
        let member = conversation_member::Entity::find()
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(conversation_member::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .one(&self.db)
            .await?;
        Ok(member.is_some())
    }
}
