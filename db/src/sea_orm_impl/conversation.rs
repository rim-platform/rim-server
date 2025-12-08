use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait,
    ColumnTrait,
    DbConn,
    EntityTrait,
    JoinType,
    QueryFilter,
    QuerySelect,
    RelationTrait,
    Set
};
use sea_orm::prelude::Uuid;
use entity::{conversation, conversation_member};
use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::repository::repository::Repository;
use rim_core::user::model::{UserId};
use crate::impl_repository_for_entity;

pub struct SeaOrmConversationRepository {
    pub db: DbConn,
}

impl SeaOrmConversationRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

impl_repository_for_entity!(
    SeaOrmConversationRepository,
    conversation::Entity,
    Conversation,
    ConversationId
);

#[async_trait]
impl ConversationRepository for SeaOrmConversationRepository {
    async fn create_conversation(
        &self,
        conversation_type: ConversationType,
    ) -> anyhow::Result<Self::Model> {
        let active = conversation::ActiveModel {
            conversation_type: Set(conversation_type.into()),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await?;
        Ok(inserted.into())
    }

    async fn get_conversations_for_user(
        &self,
        user_id: UserId,
    ) -> anyhow::Result<Vec<Self::Model>> {
        let convs = conversation::Entity::find()
            .join(
                JoinType::InnerJoin,
                conversation_member::Relation::Conversation.def(),
            )
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .all(&self.db)
            .await?;

        Ok(convs.into_iter().map(Into::into).collect())
    }
}