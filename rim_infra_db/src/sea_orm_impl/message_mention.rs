use std::collections::HashMap;

use async_trait::async_trait;
use entity::message_mention;
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message_mention::repository::MessageMentionRepository;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, JoinType, QueryFilter, QueryOrder, QuerySelect,
    RelationTrait, Set,
};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmMessageMentionRepository {
    pub db: DbHandle,
}

impl SeaOrmMessageMentionRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MessageMentionRepository for SeaOrmMessageMentionRepository {
    async fn create_mentions(&self, message_id: MessageId, user_ids: &[UserId]) -> RepoResult<()> {
        for user_id in user_ids {
            let active = message_mention::ActiveModel {
                message_id: Set(message_id.into()),
                mentioned_user_id: Set((*user_id).into()),
                ..Default::default()
            };
            active.insert(&self.db).await.map_err(map_db_err)?;
        }
        Ok(())
    }

    async fn delete_mentions_for_message(&self, message_id: MessageId) -> RepoResult<()> {
        message_mention::Entity::delete_many()
            .filter(message_mention::Column::MessageId.eq(Uuid::from(message_id)))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn get_mentions_for_messages(
        &self,
        message_ids: &[MessageId],
    ) -> RepoResult<HashMap<MessageId, Vec<UserId>>> {
        if message_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let uuids: Vec<Uuid> = message_ids.iter().copied().map(Into::into).collect();
        let models = message_mention::Entity::find()
            .filter(message_mention::Column::MessageId.is_in(uuids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        let mut result: HashMap<MessageId, Vec<UserId>> = HashMap::new();
        for m in models {
            result
                .entry(m.message_id.into())
                .or_default()
                .push(m.mentioned_user_id.into());
        }
        Ok(result)
    }

    async fn get_messages_mentioning_user(
        &self,
        user_id: UserId,
        conversation_ids: &[ConversationId],
        limit: u32,
    ) -> RepoResult<Vec<MessageId>> {
        if conversation_ids.is_empty() {
            return Ok(Vec::new());
        }

        let conv_uuids: Vec<Uuid> = conversation_ids.iter().copied().map(Into::into).collect();

        let models = message_mention::Entity::find()
            .filter(message_mention::Column::MentionedUserId.eq(Uuid::from(user_id)))
            .join(
                JoinType::InnerJoin,
                message_mention::Relation::Message.def(),
            )
            .filter(entity::message::Column::ConversationId.is_in(conv_uuids))
            .filter(entity::message::Column::IsDeleted.eq(false))
            .order_by_desc(message_mention::Column::CreatedAt)
            .limit(Some(limit as u64))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(|m| m.message_id.into()).collect())
    }
}
