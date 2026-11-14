use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use entity::{bot_token, incoming_webhook};
use rim_core::bot::model::{
    Bot, BotFeedJoin, BotFeedMessage, BotFeedPosition, BotPatch, BotToken, BotTokenHolder,
    BotTokenId, IncomingWebhook, IncomingWebhookHolder, IncomingWebhookId, NewBot, NewBotToken,
    NewIncomingWebhook, RevokedCredentials, WebhookPreset,
};
use rim_core::bot::repository::BotRepository;
use rim_core::conversation::model::ConversationId;
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
        COALESCE(c.display_name, c.username) AS added_by_name, b.created_at, b.deactivated_at,
        b.member_joins_since
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
    member_joins_since: Option<sea_orm::prelude::DateTimeWithTimeZone>,
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
            member_joins_since: row.member_joins_since.map(Into::into),
        }
    }
}

#[derive(FromQueryResult)]
struct FeedMessageRow {
    id: Uuid,
    conversation_id: Uuid,
    thread_id: Option<Uuid>,
    sent_by: Uuid,
    text: Option<String>,
    sent_at: sea_orm::prelude::DateTimeWithTimeZone,
    edited_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
    mentioned: bool,
}

#[derive(FromQueryResult)]
struct FeedJoinRow {
    id: Uuid,
    user_id: Uuid,
    joined_at: sea_orm::prelude::DateTimeWithTimeZone,
}

#[derive(FromQueryResult)]
struct NowRow {
    now: sea_orm::prelude::DateTimeWithTimeZone,
}

/// A hold-back as the seconds a SQL parameter carries.
fn seconds(hold_back: Duration) -> Value {
    (hold_back.num_milliseconds() as f64 / 1000.0).into()
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

/// A webhook row as the domain knows it.
///
/// # Errors
/// `InvalidData` for a preset this build does not know.
fn webhook_of(model: incoming_webhook::Model) -> RepoResult<IncomingWebhook> {
    let preset = WebhookPreset::parse(&model.preset).ok_or_else(|| {
        RepoError::with_message(RepoErrorKind::InvalidData, "unknown webhook preset")
    })?;
    Ok(IncomingWebhook {
        id: model.id.into(),
        server_id: model.server_id.into(),
        conversation_id: model.conversation_id.into(),
        bot_user_id: model.bot_user_id.into(),
        preset,
        template: model.template,
        created_at: model.created_at.into(),
        last_used_at: model.last_used_at.map(Into::into),
        revoked_at: model.revoked_at.map(Into::into),
    })
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
               INSERT INTO bot (user_id, server_id, run_by, reads_go_to, created_by,
                   member_joins_since)
               SELECT id, $4::uuid, $5::text, $6::text, $7::uuid,
                   CASE WHEN $8::boolean THEN now() END
               FROM new_user"#,
            vec![
                uuid(user_id),
                bot.username.into(),
                bot.name.into(),
                uuid(bot.server_id),
                bot.run_by.into(),
                bot.reads_go_to.into(),
                uuid(bot.created_by),
                bot.sees_member_joins.into(),
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
                       reads_go_to = COALESCE($4::text, reads_go_to),
                       member_joins_since = CASE $5::boolean
                           WHEN true THEN COALESCE(member_joins_since, now())
                           WHEN false THEN NULL
                           ELSE member_joins_since
                       END
                   WHERE user_id = $1"#,
                vec![
                    uuid(user_id),
                    patch.name.into(),
                    patch.run_by.into(),
                    patch.reads_go_to.into(),
                    patch.sees_member_joins.into(),
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
    async fn create_webhook(&self, webhook: NewIncomingWebhook) -> RepoResult<IncomingWebhook> {
        let active = incoming_webhook::ActiveModel {
            id: Set(IncomingWebhookId::new().into()),
            server_id: Set(webhook.server_id.into()),
            conversation_id: Set(webhook.conversation_id.into()),
            bot_user_id: Set(webhook.bot_user_id.into()),
            preset: Set(webhook.preset.as_str().to_string()),
            template: Set(webhook.template),
            token_hash: Set(webhook.token_hash),
            created_by: Set(webhook.created_by.into()),
            ..Default::default()
        };
        webhook_of(active.insert(&self.db).await.map_err(map_db_err)?)
    }

    async fn find_webhook(&self, id: IncomingWebhookId) -> RepoResult<Option<IncomingWebhook>> {
        incoming_webhook::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .map(webhook_of)
            .transpose()
    }

    async fn find_active_webhook(
        &self,
        conversation_id: ConversationId,
    ) -> RepoResult<Option<IncomingWebhook>> {
        incoming_webhook::Entity::find()
            .filter(incoming_webhook::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .filter(incoming_webhook::Column::RevokedAt.is_null())
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .map(webhook_of)
            .transpose()
    }

    async fn list_webhooks(&self, server_id: ServerId) -> RepoResult<Vec<IncomingWebhook>> {
        incoming_webhook::Entity::find()
            .filter(incoming_webhook::Column::ServerId.eq::<Uuid>(server_id.into()))
            .order_by_desc(incoming_webhook::Column::CreatedAt)
            .order_by_desc(incoming_webhook::Column::Id)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(webhook_of)
            .collect()
    }

    async fn revoke_webhook(
        &self,
        id: IncomingWebhookId,
        at: DateTime<Utc>,
    ) -> RepoResult<Option<IncomingWebhook>> {
        self.execute(
            "UPDATE incoming_webhook SET revoked_at = $2 WHERE id = $1 AND revoked_at IS NULL",
            vec![uuid(id), utc(at)],
        )
        .await?;
        self.find_webhook(id).await
    }

    async fn update_webhook_format(
        &self,
        id: IncomingWebhookId,
        preset: WebhookPreset,
        template: Option<serde_json::Value>,
    ) -> RepoResult<Option<IncomingWebhook>> {
        let changed = self
            .execute(
                "UPDATE incoming_webhook SET preset = $2, template = $3 \
                 WHERE id = $1 AND revoked_at IS NULL",
                vec![
                    uuid(id),
                    preset.as_str().into(),
                    Value::Json(template.map(Box::new)),
                ],
            )
            .await?;
        if changed == 0 {
            return Ok(None);
        }
        self.find_webhook(id).await
    }

    async fn revoke_all_webhooks(&self, bot_user_id: UserId, at: DateTime<Utc>) -> RepoResult<u64> {
        self.execute(
            "UPDATE incoming_webhook SET revoked_at = $2 \
             WHERE bot_user_id = $1 AND revoked_at IS NULL",
            vec![uuid(bot_user_id), utc(at)],
        )
        .await
    }

    async fn revoke_held_by(
        &self,
        user_id: UserId,
        server_id: Option<ServerId>,
        at: DateTime<Utc>,
    ) -> RepoResult<RevokedCredentials> {
        let values = || vec![uuid(user_id), server_id.map(Uuid::from).into(), utc(at)];
        let tokens = self
            .execute(
                "UPDATE bot_token t SET revoked_at = $3 FROM bot b \
                 WHERE b.user_id = t.bot_user_id AND t.revoked_at IS NULL \
                     AND (t.created_by = $1 OR b.created_by = $1) \
                     AND ($2::uuid IS NULL OR b.server_id = $2)",
                values(),
            )
            .await?;
        let webhooks = self
            .execute(
                "UPDATE incoming_webhook w SET revoked_at = $3 FROM bot b \
                 WHERE b.user_id = w.bot_user_id AND w.revoked_at IS NULL \
                     AND (w.created_by = $1 OR b.created_by = $1) \
                     AND ($2::uuid IS NULL OR w.server_id = $2)",
                values(),
            )
            .await?;
        Ok(RevokedCredentials { tokens, webhooks })
    }

    async fn find_webhook_holder(
        &self,
        token_hash: &str,
    ) -> RepoResult<Option<IncomingWebhookHolder>> {
        let found = incoming_webhook::Entity::find()
            .filter(incoming_webhook::Column::TokenHash.eq(token_hash))
            .find_also_related(entity::bot::Entity)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        let Some((model, Some(bot))) = found else {
            return Ok(None);
        };
        Ok(Some(IncomingWebhookHolder {
            webhook: webhook_of(model)?,
            bot_deactivated_at: bot.deactivated_at.map(Into::into),
        }))
    }

    async fn touch_webhook(&self, id: IncomingWebhookId, at: DateTime<Utc>) -> RepoResult<()> {
        self.execute(
            "UPDATE incoming_webhook SET last_used_at = $2 WHERE id = $1",
            vec![uuid(id), utc(at)],
        )
        .await?;
        Ok(())
    }

    async fn feed_now(&self) -> RepoResult<DateTime<Utc>> {
        let row = NowRow::find_by_statement(statement("SELECT now() AS now", vec![]))
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
        Ok(row.now.into())
    }

    async fn list_feed_messages(
        &self,
        bot_user_id: UserId,
        server_id: ServerId,
        after: BotFeedPosition,
        hold_back: Duration,
        limit: u64,
    ) -> RepoResult<Vec<BotFeedMessage>> {
        // One bounded walk of `(conversation_id, sent_at, id)` per channel, so a poll costs the
        // same however much history lies behind or ahead of the cursor.
        let rows = FeedMessageRow::find_by_statement(statement(
            r#"SELECT m.id, m.conversation_id, m.thread_id, m.sent_by, m.text, m.sent_at,
                   m.edited_at,
                   EXISTS (SELECT 1 FROM message_mention mm
                           WHERE mm.message_id = m.id AND mm.mentioned_user_id = $1) AS mentioned
               FROM conversation_member cm
               JOIN conversation c ON c.id = cm.conversation_id
               CROSS JOIN LATERAL (
                   SELECT id, conversation_id, thread_id, sent_by, text, sent_at, edited_at
                   FROM message
                   WHERE conversation_id = c.id
                     AND thread_id IS NULL
                     AND is_deleted = false
                     AND sent_by <> $1
                     AND sent_at >= cm.joined_at
                     AND sent_at <= now() - make_interval(secs => $5::double precision)
                     AND (sent_at, id) > ($3::timestamptz, $4::uuid)
                   ORDER BY sent_at, id
                   LIMIT $6
               ) m
               WHERE cm.user_id = $1
                 AND cm.pending_since IS NULL
                 AND c.server_id = $2
                 AND c.conversation_type = 'channel'
                 AND c.encrypted_at IS NULL
                 AND c.archived_at IS NULL
               ORDER BY m.sent_at, m.id
               LIMIT $6"#,
            vec![
                uuid(bot_user_id),
                uuid(server_id),
                utc(after.at),
                after.id.into(),
                seconds(hold_back),
                (limit as i64).into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(rows
            .into_iter()
            .map(|row| BotFeedMessage {
                id: row.id.into(),
                conversation_id: row.conversation_id.into(),
                thread_id: row.thread_id.map(Into::into),
                sent_by: row.sent_by.into(),
                text: row.text,
                sent_at: row.sent_at.into(),
                edited_at: row.edited_at.map(Into::into),
                mentioned: row.mentioned,
            })
            .collect())
    }

    async fn list_feed_joins(
        &self,
        bot_user_id: UserId,
        server_id: ServerId,
        after: BotFeedPosition,
        hold_back: Duration,
        limit: u64,
    ) -> RepoResult<Vec<BotFeedJoin>> {
        let rows = FeedJoinRow::find_by_statement(statement(
            r#"SELECT sm.id, sm.user_id, sm.joined_at
               FROM server_member sm
               JOIN bot b ON b.user_id = $1 AND b.server_id = sm.server_id
               WHERE sm.server_id = $2
                 AND sm.user_id <> $1
                 AND sm.joined_at >= b.member_joins_since
                 AND sm.joined_at <= now() - make_interval(secs => $5::double precision)
                 AND (sm.joined_at, sm.id) > ($3::timestamptz, $4::uuid)
               ORDER BY sm.joined_at, sm.id
               LIMIT $6"#,
            vec![
                uuid(bot_user_id),
                uuid(server_id),
                utc(after.at),
                after.id.into(),
                seconds(hold_back),
                (limit as i64).into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(rows
            .into_iter()
            .map(|row| BotFeedJoin {
                member_id: row.id.into(),
                user_id: row.user_id.into(),
                joined_at: row.joined_at.into(),
            })
            .collect())
    }
}
