use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait,
    ColumnTrait,
    DbConn,
    EntityTrait,
    JoinType,
    QueryFilter,
    QuerySelect,
    QueryOrder,
    RelationTrait,
    Set
};
use entity::{conversation, conversation_member};
use entity::sea_orm_active_enums::ConversationType;
use rim_core::repositories::{conversation_repository::ConversationRepository};
use rim_core::repositories::repository::Repository;
use crate::impl_repository_for_entity;

pub struct SeaOrmConversationRepository {
    pub db: DbConn,
}

impl SeaOrmConversationRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

type Id = i32;

impl_repository_for_entity!(
    SeaOrmConversationRepository,
    conversation::Model,
    conversation::Column,
    Id,
    conversation::Entity
);

#[async_trait]
impl ConversationRepository<conversation::Model, Id, ConversationType> for SeaOrmConversationRepository {
    async fn create_conversation(
        &self,
        conversation_type: ConversationType,
    ) -> anyhow::Result<conversation::Model> {
        let active = conversation::ActiveModel {
            conversation_type: Set(conversation_type),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await?;
        Ok(inserted)
    }

    async fn get_conversations_for_user(
        &self,
        user_id: i32,
    ) -> anyhow::Result<Vec<conversation::Model>> {
        let convs = conversation::Entity::find()
            .join(
                JoinType::InnerJoin,
                conversation_member::Relation::Conversation.def(),
            )
            .filter(conversation_member::Column::UserId.eq(user_id))
            .all(&self.db)
            .await?;

        Ok(convs)
    }
    }