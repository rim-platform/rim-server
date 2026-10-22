use std::collections::HashMap;

use async_trait::async_trait;
use entity::message_reaction;
use rim_core::message::model::MessageId;
use rim_core::message_reaction::model::{MessageReaction, MessageReactionId, NewMessageReaction};
use rim_core::message_reaction::repository::MessageReactionRepository;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, ModelTrait,
    QueryFilter, Set, Statement,
};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmMessageReactionRepository {
    pub db: DbHandle,
}

impl SeaOrmMessageReactionRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MessageReactionRepository for SeaOrmMessageReactionRepository {
    async fn lock_message(&self, message_id: MessageId) -> RepoResult<bool> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM message WHERE id = $1 FOR UPDATE",
                [Uuid::from(message_id).into()],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(row.is_some())
    }

    async fn add_reaction(&self, reaction: NewMessageReaction) -> RepoResult<MessageReaction> {
        let active = message_reaction::ActiveModel {
            id: Set(MessageReactionId::new().into()),
            message_id: Set(reaction.message_id.into()),
            user_id: Set(reaction.user_id.into()),
            emoji: Set(reaction.emoji),
            created_at: Set(chrono::Utc::now().into()),
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn remove_reaction(
        &self,
        message_id: MessageId,
        user_id: UserId,
        emoji: &str,
    ) -> RepoResult<()> {
        let model = message_reaction::Entity::find()
            .filter(message_reaction::Column::MessageId.eq::<Uuid>(message_id.into()))
            .filter(message_reaction::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(message_reaction::Column::Emoji.eq(emoji))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        if let Some(model) = model {
            model.delete(&self.db).await.map_err(map_db_err)?;
        }

        Ok(())
    }

    async fn find_reaction(
        &self,
        message_id: MessageId,
        user_id: UserId,
        emoji: &str,
    ) -> RepoResult<Option<MessageReaction>> {
        let model = message_reaction::Entity::find()
            .filter(message_reaction::Column::MessageId.eq::<Uuid>(message_id.into()))
            .filter(message_reaction::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(message_reaction::Column::Emoji.eq(emoji))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn get_reactions_for_message(
        &self,
        message_id: MessageId,
    ) -> RepoResult<Vec<MessageReaction>> {
        let reactions = message_reaction::Entity::find()
            .filter(message_reaction::Column::MessageId.eq::<Uuid>(message_id.into()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(reactions.into_iter().map(Into::into).collect())
    }

    async fn get_reactions_for_messages(
        &self,
        message_ids: &[MessageId],
    ) -> RepoResult<HashMap<MessageId, Vec<MessageReaction>>> {
        if message_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let ids = message_ids
            .iter()
            .copied()
            .map(Into::into)
            .collect::<Vec<Uuid>>();

        let reactions = message_reaction::Entity::find()
            .filter(message_reaction::Column::MessageId.is_in(ids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        let mut out: HashMap<MessageId, Vec<MessageReaction>> = HashMap::new();
        for reaction in reactions.into_iter().map(Into::<MessageReaction>::into) {
            out.entry(reaction.message_id).or_default().push(reaction);
        }
        Ok(out)
    }
}
