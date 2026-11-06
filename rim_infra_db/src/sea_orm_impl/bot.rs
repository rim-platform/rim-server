use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::bot_token;
use rim_core::bot::model::{
    Bot, BotPatch, BotToken, BotTokenHolder, BotTokenId, NewBot, NewBotToken,
};
use rim_core::bot::repository::BotRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, FromQueryResult,
    PaginatorTrait, QueryFilter, QueryOrder, Set, Statement, Value,
};

use crate::DbHandle;
use crate::error::map_db_err;

/// A bot with its name and the name of whoever added it.
const SELECT_BOT: &str = r#"SELECT b.user_id, b.server_id,
        COALESCE(u.display_name, u.username) AS name, b.run_by, b.reads_go_to, b.created_by,
        COALESCE(c.display_name, c.username) AS added_by_name, b.created_at, b.deactivated_at
    FROM bot b
    JOIN "user" u ON u.id = b.user_id
    JOIN "user" c ON c.id = b.created_by"#;

fn statement(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

fn uuid<T: Into<Uuid>>(id: T) -> Value {
    let id: Uuid = id.into();
    id.into()
}

fn utc(at: DateTime<Utc>) -> Value {
    sea_orm::prelude::DateTimeWithTimeZone::from(at).into()
}

#[derive(FromQueryResult)]
struct BotRow {
    user_id: Uuid,
    server_id: Uuid,
    name: String,
    run_by: String,
    reads_go_to: String,
    created_by: Uuid,
    added_by_name: Option<String>,
    created_at: sea_orm::prelude::DateTimeWithTimeZone,
    deactivated_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
}

impl From<BotRow> for Bot {
    fn from(row: BotRow) -> Self {
        Self {
            user_id: row.user_id.into(),
            server_id: row.server_id.into(),
            name: row.name,
            run_by: row.run_by,
            reads_go_to: row.reads_go_to,
            created_by: row.created_by.into(),
            added_by_name: row.added_by_name,
            created_at: row.created_at.into(),
            deactivated_at: row.deactivated_at.map(Into::into),
        }
    }
}

#[derive(FromQueryResult)]
struct IdRow {
    id: Uuid,
}

#[derive(FromQueryResult)]
struct TokenHolderRow {
    id: Uuid,
    bot_user_id: Uuid,
    label: String,
    created_at: sea_orm::prelude::DateTimeWithTimeZone,
    last_used_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
    revoked_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
    server_id: Uuid,
    bot_deactivated_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
}

pub struct SeaOrmBotRepository {
    db: DbHandle,
}

impl SeaOrmBotRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    async fn execute(&self, sql: &str, values: Vec<Value>) -> RepoResult<u64> {
        Ok(self
            .db
            .execute_raw(statement(sql, values))
            .await
            .map_err(map_db_err)?
            .rows_affected())
    }
}

#[async_trait]
impl BotRepository for SeaOrmBotRepository {
    async fn create_bot(&self, bot: NewBot) -> RepoResult<Bot> {
        let user_id = UserId::new();
        self.execute(
            r#"WITH new_user AS (
                   INSERT INTO "user" (id, username, display_name, kind)
                   VALUES ($1, $2, $3, 'bot')
                   RETURNING id
               )
               INSERT INTO bot (user_id, server_id, run_by, reads_go_to, created_by)
               SELECT id, $4::uuid, $5::text, $6::text, $7::uuid FROM new_user"#,
            vec![
                uuid(user_id),
                bot.username.into(),
                bot.name.into(),
                uuid(bot.server_id),
                bot.run_by.into(),
                bot.reads_go_to.into(),
                uuid(bot.created_by),
            ],
        )
        .await?;
        self.find_bot(user_id)
            .await?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))
    }

    async fn find_bot(&self, user_id: UserId) -> RepoResult<Option<Bot>> {
        let row = BotRow::find_by_statement(statement(
            &format!("{SELECT_BOT} WHERE b.user_id = $1"),
            vec![uuid(user_id)],
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(row.map(Into::into))
    }

    async fn list_by_server(&self, server_id: ServerId) -> RepoResult<Vec<Bot>> {
        let rows = BotRow::find_by_statement(statement(
            &format!("{SELECT_BOT} WHERE b.server_id = $1 ORDER BY b.created_at, b.user_id"),
            vec![uuid(server_id)],
        ))
        .all(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn list_by_users(&self, user_ids: &[UserId]) -> RepoResult<Vec<Bot>> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<Uuid> = user_ids.iter().copied().map(Into::into).collect();
        let rows = BotRow::find_by_statement(statement(
            &format!("{SELECT_BOT} WHERE b.user_id = ANY($1)"),
            vec![ids.into()],
        ))
        .all(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn bots_among(&self, user_ids: &[UserId]) -> RepoResult<Vec<UserId>> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<Uuid> = user_ids.iter().copied().map(Into::into).collect();
        let rows = IdRow::find_by_statement(statement(
            r#"SELECT id FROM "user" WHERE id = ANY($1) AND kind = 'bot'"#,
            vec![ids.into()],
        ))
        .all(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(rows.into_iter().map(|row| row.id.into()).collect())
    }

    async fn count_active(&self, server_id: ServerId) -> RepoResult<u64> {
        entity::bot::Entity::find()
            .filter(entity::bot::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(entity::bot::Column::DeactivatedAt.is_null())
            .count(&self.db)
            .await
            .map_err(map_db_err)
    }

    async fn lock_server_bots(&self, server_id: ServerId) -> RepoResult<()> {
        self.execute(
            "SELECT pg_advisory_xact_lock(hashtextextended('bot.server.' || $1::text, 0))",
            vec![uuid(server_id)],
        )
        .await?;
        Ok(())
    }

    async fn lock_bot(&self, user_id: UserId) -> RepoResult<bool> {
        let row = IdRow::find_by_statement(statement(
            "SELECT user_id AS id FROM bot WHERE user_id = $1 FOR NO KEY UPDATE",
            vec![uuid(user_id)],
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(row.is_some())
    }

    async fn update_bot(&self, user_id: UserId, patch: BotPatch) -> RepoResult<Bot> {
        let updated = self
            .execute(
                r#"WITH renamed AS (
                       UPDATE "user" SET display_name = $2, updated_at = now()
                       WHERE id = $1 AND kind = 'bot' AND $2::text IS NOT NULL
                   )
                   UPDATE bot SET run_by = COALESCE($3::text, run_by),
                       reads_go_to = COALESCE($4::text, reads_go_to)
                   WHERE user_id = $1"#,
                vec![
                    uuid(user_id),
                    patch.name.into(),
                    patch.run_by.into(),
                    patch.reads_go_to.into(),
                ],
            )
            .await?;
        if updated == 0 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }
        self.find_bot(user_id)
            .await?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))
    }

    async fn set_deactivated(&self, user_id: UserId, at: DateTime<Utc>) -> RepoResult<bool> {
        let updated = self
            .execute(
                "UPDATE bot SET deactivated_at = $2 WHERE user_id = $1 AND deactivated_at IS NULL",
                vec![uuid(user_id), utc(at)],
            )
            .await?;
        Ok(updated == 1)
    }

    async fn create_token(&self, token: NewBotToken) -> RepoResult<BotToken> {
        let active = bot_token::ActiveModel {
            id: Set(BotTokenId::new().into()),
            bot_user_id: Set(token.bot_user_id.into()),
            label: Set(token.label),
            token_hash: Set(token.token_hash),
            created_by: Set(token.created_by.into()),
            ..Default::default()
        };
        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn find_token(&self, id: BotTokenId) -> RepoResult<Option<BotToken>> {
        let model = bot_token::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn list_tokens(&self, bot_user_id: UserId) -> RepoResult<Vec<BotToken>> {
        let models = bot_token::Entity::find()
            .filter(bot_token::Column::BotUserId.eq::<Uuid>(bot_user_id.into()))
            .order_by_desc(bot_token::Column::CreatedAt)
            .order_by_desc(bot_token::Column::Id)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn count_active_tokens(&self, bot_user_id: UserId) -> RepoResult<u64> {
        bot_token::Entity::find()
            .filter(bot_token::Column::BotUserId.eq::<Uuid>(bot_user_id.into()))
            .filter(bot_token::Column::RevokedAt.is_null())
            .count(&self.db)
            .await
            .map_err(map_db_err)
    }

    async fn revoke_token(
        &self,
        id: BotTokenId,
        at: DateTime<Utc>,
    ) -> RepoResult<Option<BotToken>> {
        self.execute(
            "UPDATE bot_token SET revoked_at = $2 WHERE id = $1 AND revoked_at IS NULL",
            vec![uuid(id), utc(at)],
        )
        .await?;
        self.find_token(id).await
    }

    async fn revoke_all_tokens(&self, bot_user_id: UserId, at: DateTime<Utc>) -> RepoResult<u64> {
        self.execute(
            "UPDATE bot_token SET revoked_at = $2 WHERE bot_user_id = $1 AND revoked_at IS NULL",
            vec![uuid(bot_user_id), utc(at)],
        )
        .await
    }

    async fn find_token_holder(&self, token_hash: &str) -> RepoResult<Option<BotTokenHolder>> {
        let row = TokenHolderRow::find_by_statement(statement(
            "SELECT t.id, t.bot_user_id, t.label, t.created_at, t.last_used_at, t.revoked_at, \
                    b.server_id, b.deactivated_at AS bot_deactivated_at \
             FROM bot_token t JOIN bot b ON b.user_id = t.bot_user_id \
             WHERE t.token_hash = $1",
            vec![token_hash.into()],
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(row.map(|row| BotTokenHolder {
            token: BotToken {
                id: row.id.into(),
                bot_user_id: row.bot_user_id.into(),
                label: row.label,
                created_at: row.created_at.into(),
                last_used_at: row.last_used_at.map(Into::into),
                revoked_at: row.revoked_at.map(Into::into),
            },
            server_id: row.server_id.into(),
            bot_deactivated_at: row.bot_deactivated_at.map(Into::into),
        }))
    }

    async fn touch_token(&self, id: BotTokenId, at: DateTime<Utc>) -> RepoResult<()> {
        self.execute(
            "UPDATE bot_token SET last_used_at = $2 WHERE id = $1",
            vec![uuid(id), utc(at)],
        )
        .await?;
        Ok(())
    }
}
