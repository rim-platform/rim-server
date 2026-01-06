use crate::impl_repository_for_entity;
use async_trait::async_trait;
use entity::{conversation, conversation_member};
use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::repository::repository::Repository;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DbConn, EntityTrait, JoinType, QueryFilter, QuerySelect,
    RelationTrait, Set,
};

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
        server_id: ServerId,
    ) -> anyhow::Result<Self::Model> {
        let active = conversation::ActiveModel {
            conversation_type: Set(conversation_type.into()),
            server_id: Set(server_id.into()),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await?;
        Ok(inserted.into())
    }

    async fn get_conversations_for_user(
        &self,
        user_id: UserId,
        server_id: ServerId,
    ) -> anyhow::Result<Vec<Self::Model>> {
        let conversations = conversation::Entity::find()
            .join(
                JoinType::InnerJoin,
                conversation_member::Relation::Conversation.def(),
            )
            .filter(conversation_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(conversation::Column::ServerId.eq::<Uuid>(server_id.into()))
            .all(&self.db)
            .await?;

        Ok(conversations.into_iter().map(Into::into).collect())
    }
}
