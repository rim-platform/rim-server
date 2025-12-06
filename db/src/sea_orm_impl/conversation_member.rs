use sea_orm::ColumnTrait;
use sea_orm::QueryOrder;
use async_trait::async_trait;
use crate::impl_repository_for_entity;
use entity::conversation_member;
use rim_core::repositories::repository::Repository;
use sea_orm::{ActiveModelTrait, DbConn, EntityTrait, JoinType, QueryFilter, QuerySelect, RelationTrait, Set};
use rim_core::repositories::conversation_member_repository::ConversationMemberRepository;

pub struct SeaOrmConversationMemberRepository {
    db: DbConn,
}

impl SeaOrmConversationMemberRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}


type Id = i32;

impl_repository_for_entity!(
    SeaOrmConversationMemberRepository,
    conversation_member::Model,
    conversation_member::Column,
    Id,
    conversation_member::Entity
);


#[async_trait]
impl ConversationMemberRepository<conversation_member::Model, Id>
for SeaOrmConversationMemberRepository {
    async fn add_member(
        &self,
        conversation_id: Id,
        user_id: Id,
    ) -> anyhow::Result<conversation_member::Model> {
        let active = conversation_member::ActiveModel {
            conversation_id: Set(conversation_id),
            user_id: Set(user_id),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await?;
        Ok(inserted)
    }

    async fn remove_member(
        &self,
        conversation_id: Id,
        user_id: Id,
    ) -> anyhow::Result<()> {
        conversation_member::Entity::delete_many()
            .filter(conversation_member::Column::ConversationId.eq(conversation_id))
            .filter(conversation_member::Column::UserId.eq(user_id))
            .exec(&self.db)
            .await?;
        Ok(())
    }

    async fn get_members_for_conversation(
        &self,
        conversation_id: Id,
    ) -> anyhow::Result<Vec<conversation_member::Model>> {
        let members = conversation_member::Entity::find()
            .filter(conversation_member::Column::ConversationId.eq(conversation_id))
            .all(&self.db)
            .await?;
        Ok(members)
    }
}
