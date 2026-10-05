use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::conversation_read_state;
use rim_core::conversation::model::ConversationId;
use rim_core::conversation_read_state::model::ConversationReadState;
use rim_core::conversation_read_state::repository::ConversationReadStateRepository;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Statement};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmConversationReadStateRepository {
    pub db: DbHandle,
}

impl SeaOrmConversationReadStateRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ConversationReadStateRepository for SeaOrmConversationReadStateRepository {
    async fn upsert(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
        last_read_at: DateTime<Utc>,
    ) -> RepoResult<()> {
        let user_uuid: Uuid = user_id.into();
        let conv_uuid: Uuid = conversation_id.into();
        let ts: chrono::DateTime<chrono::FixedOffset> = last_read_at.into();

        // UPSERT with forward-only guard: never move last_read_at backwards.
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"INSERT INTO conversation_read_state (user_id, conversation_id, last_read_at)
                   VALUES ($1, $2, $3)
                   ON CONFLICT (user_id, conversation_id)
                   DO UPDATE SET last_read_at = EXCLUDED.last_read_at
                   WHERE conversation_read_state.last_read_at < EXCLUDED.last_read_at"#,
                [user_uuid.into(), conv_uuid.into(), ts.into()],
            ))
            .await
            .map_err(map_db_err)?;

        Ok(())
    }

    async fn get(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
    ) -> RepoResult<Option<ConversationReadState>> {
        let model = conversation_read_state::Entity::find()
            .filter(conversation_read_state::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(
                conversation_read_state::Column::ConversationId.eq::<Uuid>(conversation_id.into()),
            )
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn get_unread_counts(
        &self,
        user_id: UserId,
        conversation_ids: &[ConversationId],
    ) -> RepoResult<Vec<(ConversationId, i64)>> {
        if conversation_ids.is_empty() {
            return Ok(vec![]);
        }

        let user_uuid: Uuid = user_id.into();
        let conv_uuids: Vec<Uuid> = conversation_ids.iter().copied().map(Into::into).collect();

        // Baseline: the last read, else when the user joined (conversation_member.joined_at),
        // else conversation.created_at for a conversation without a member row (callers pass
        // joined conversations only since Track 1.1, so that fallback is defensive).
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"SELECT c.id AS conversation_id, COUNT(m.id) AS unread_count
                   FROM conversation c
                   LEFT JOIN conversation_member cm
                       ON cm.user_id = $1 AND cm.conversation_id = c.id
                   LEFT JOIN conversation_read_state crs
                       ON crs.user_id = $1 AND crs.conversation_id = c.id
                   LEFT JOIN message m
                       ON m.conversation_id = c.id
                       AND m.sent_at > COALESCE(crs.last_read_at, cm.joined_at, c.created_at)
                       AND m.thread_id IS NULL
                       AND m.is_deleted = false
                       AND m.sent_by != $1
                   WHERE c.id = ANY($2)
                   GROUP BY c.id
                   HAVING COUNT(m.id) > 0"#,
                [user_uuid.into(), conv_uuids.into()],
            ))
            .await
            .map_err(map_db_err)?;

        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let conv_id: Uuid = row.try_get("", "conversation_id").map_err(map_db_err)?;
            let count: i64 = row.try_get("", "unread_count").map_err(map_db_err)?;
            result.push((conv_id.into(), count));
        }

        Ok(result)
    }

    async fn mark_all_read(
        &self,
        user_id: UserId,
        conversation_ids: &[ConversationId],
        read_at: DateTime<Utc>,
    ) -> RepoResult<()> {
        if conversation_ids.is_empty() {
            return Ok(());
        }

        let user_uuid: Uuid = user_id.into();
        let conv_uuids: Vec<Uuid> = conversation_ids.iter().copied().map(Into::into).collect();
        let ts: chrono::DateTime<chrono::FixedOffset> = read_at.into();

        // Bulk UPSERT with forward-only guard.
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"INSERT INTO conversation_read_state (user_id, conversation_id, last_read_at)
                   SELECT $1, unnest($2::uuid[]), $3
                   ON CONFLICT (user_id, conversation_id)
                   DO UPDATE SET last_read_at = EXCLUDED.last_read_at
                   WHERE conversation_read_state.last_read_at < EXCLUDED.last_read_at"#,
                [user_uuid.into(), conv_uuids.into(), ts.into()],
            ))
            .await
            .map_err(map_db_err)?;

        Ok(())
    }
}
