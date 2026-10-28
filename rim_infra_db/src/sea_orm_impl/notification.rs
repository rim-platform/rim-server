use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::{user_notification, user_notification_settings};
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::notification::model::{
    DigestCandidate, DigestFrequency, DndPause, NewReactionNotification, NewUserNotification,
    NotificationReason, StoredReactionNotification, UserNotification, UserNotificationId,
    UserNotificationSettings,
};
use rim_core::notification::repository::{
    ListUserNotificationsParams, NotificationRepository, UserNotificationCursor,
};
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::repository::pagination::{Page, PageDir};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DbBackend, EntityTrait, FromQueryResult,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, Statement,
};

use crate::DbHandle;
use crate::error::map_db_err;
use crate::sea_orm_impl::relay::hidden;

pub struct SeaOrmNotificationRepository {
    db: DbHandle,
}

impl SeaOrmNotificationRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

/// Leaves out a user's notifications for conversations hidden from them by the request inbox or
/// a block.
fn not_hidden(user_id: UserId) -> sea_orm::sea_query::SimpleExpr {
    sea_orm::sea_query::Expr::cust_with_values(
        format!(
            "NOT {}",
            hidden("$1", r#""user_notification"."conversation_id""#)
        ),
        [sea_orm::Value::from(Uuid::from(user_id))],
    )
}

fn uuids<T: Copy + Into<Uuid>>(ids: &[T]) -> Vec<Uuid> {
    ids.iter().copied().map(Into::into).collect()
}

#[async_trait]
impl NotificationRepository for SeaOrmNotificationRepository {
    async fn get_settings(
        &self,
        user_id: UserId,
        server_id: ServerId,
    ) -> RepoResult<Option<UserNotificationSettings>> {
        let model = user_notification_settings::Entity::find_by_id((
            Uuid::from(user_id),
            Uuid::from(server_id),
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn get_settings_for_users(
        &self,
        server_id: ServerId,
        user_ids: &[UserId],
    ) -> RepoResult<Vec<UserNotificationSettings>> {
        if user_ids.is_empty() {
            return Ok(vec![]);
        }
        let models = user_notification_settings::Entity::find()
            .filter(user_notification_settings::Column::ServerId.eq(Uuid::from(server_id)))
            .filter(user_notification_settings::Column::UserId.is_in(uuids(user_ids)))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn upsert_settings(
        &self,
        settings: UserNotificationSettings,
    ) -> RepoResult<UserNotificationSettings> {
        let (dnd_start, dnd_end, dnd_timezone) = match settings.dnd {
            Some(w) => (Some(w.start), Some(w.end), Some(w.timezone)),
            None => (None, None, None),
        };
        let (dnd_paused, dnd_paused_until) = match settings.dnd_pause {
            DndPause::Off => (false, None),
            DndPause::Indefinite => (true, None),
            DndPause::Until(until) => (true, Some(DateTimeWithTimeZone::from(until))),
        };
        let active = user_notification_settings::ActiveModel {
            user_id: Set(settings.user_id.into()),
            server_id: Set(settings.server_id.into()),
            default_level: Set(settings.default_level.into()),
            keywords: Set(settings.keywords),
            dnd_start: Set(dnd_start),
            dnd_end: Set(dnd_end),
            dnd_timezone: Set(dnd_timezone),
            dnd_paused: Set(dnd_paused),
            dnd_paused_until: Set(dnd_paused_until),
            digest: Set(settings.digest.into()),
            updated_at: Set(settings.updated_at.into()),
            reactions: Set(settings.reactions),
        };
        let model = user_notification_settings::Entity::insert(active)
            .on_conflict(
                OnConflict::columns([
                    user_notification_settings::Column::UserId,
                    user_notification_settings::Column::ServerId,
                ])
                .update_columns([
                    user_notification_settings::Column::DefaultLevel,
                    user_notification_settings::Column::Keywords,
                    user_notification_settings::Column::DndStart,
                    user_notification_settings::Column::DndEnd,
                    user_notification_settings::Column::DndTimezone,
                    user_notification_settings::Column::DndPaused,
                    user_notification_settings::Column::DndPausedUntil,
                    user_notification_settings::Column::Digest,
                    user_notification_settings::Column::UpdatedAt,
                    user_notification_settings::Column::Reactions,
                ])
                .to_owned(),
            )
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.into())
    }

    async fn create_notifications(
        &self,
        notifications: &[NewUserNotification],
        created_at: DateTime<Utc>,
    ) -> RepoResult<Vec<UserNotification>> {
        if notifications.is_empty() {
            return Ok(vec![]);
        }
        let created_at: DateTimeWithTimeZone = created_at.into();
        let ids: Vec<Uuid> = notifications
            .iter()
            .map(|_| UserNotificationId::new().into())
            .collect();
        let user_ids: Vec<Uuid> = notifications.iter().map(|n| n.user_id.into()).collect();
        let server_ids: Vec<Uuid> = notifications.iter().map(|n| n.server_id.into()).collect();
        let conversation_ids: Vec<Uuid> = notifications
            .iter()
            .map(|n| n.conversation_id.into())
            .collect();
        let message_ids: Vec<Uuid> = notifications.iter().map(|n| n.message_id.into()).collect();
        let reasons: Vec<String> = notifications
            .iter()
            .map(|n| n.reason.as_str().to_string())
            .collect();
        // One statement for every recipient; a `(user_id, message_id)` that already has a row is
        // skipped, so only the inserted rows come back.
        let models = user_notification::Model::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO user_notification
                   (id, user_id, server_id, conversation_id, message_id, reason, created_at)
               SELECT t.id, t.user_id, t.server_id, t.conversation_id, t.message_id,
                      t.reason::notification_reason, $7
               FROM unnest($1::uuid[], $2::uuid[], $3::uuid[], $4::uuid[], $5::uuid[], $6::text[])
                    AS t(id, user_id, server_id, conversation_id, message_id, reason)
               ON CONFLICT (user_id, message_id) DO NOTHING
               RETURNING id, user_id, server_id, conversation_id, message_id, reason, created_at,
                         read_at, emailed_at, reaction_emoji, reaction_by"#,
            [
                ids.into(),
                user_ids.into(),
                server_ids.into(),
                conversation_ids.into(),
                message_ids.into(),
                reasons.into(),
                created_at.into(),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn upsert_reaction_notification(
        &self,
        notification: NewReactionNotification,
        created_at: DateTime<Utc>,
    ) -> RepoResult<Option<StoredReactionNotification>> {
        let created_at: DateTimeWithTimeZone = created_at.into();
        // The conditional update leaves a row with another reason alone and returns nothing.
        // `prev` reads the row as it was before the statement, so `replaced_unread_at` is the
        // replaced row's `created_at` while it was unread.
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"WITH prev AS (
                       SELECT created_at FROM user_notification
                       WHERE user_id = $2 AND message_id = $5
                         AND reason = $6::notification_reason AND read_at IS NULL
                   ), up AS (
                       INSERT INTO user_notification
                           (id, user_id, server_id, conversation_id, message_id, reason,
                            created_at, reaction_emoji, reaction_by)
                       VALUES ($1, $2, $3, $4, $5, $6::notification_reason, $7, $8, $9)
                       ON CONFLICT (user_id, message_id) DO UPDATE
                       SET reaction_emoji = EXCLUDED.reaction_emoji,
                           reaction_by = EXCLUDED.reaction_by,
                           created_at = EXCLUDED.created_at,
                           read_at = NULL
                       WHERE user_notification.reason = EXCLUDED.reason
                       RETURNING id, user_id, server_id, conversation_id, message_id, reason,
                                 created_at, read_at, emailed_at, reaction_emoji, reaction_by
                   )
                   SELECT up.*, (SELECT created_at FROM prev) AS replaced_unread_at FROM up"#,
                [
                    Uuid::from(UserNotificationId::new()).into(),
                    Uuid::from(notification.user_id).into(),
                    Uuid::from(notification.server_id).into(),
                    Uuid::from(notification.conversation_id).into(),
                    Uuid::from(notification.message_id).into(),
                    NotificationReason::Reaction.as_str().into(),
                    created_at.into(),
                    notification.reaction.emoji.into(),
                    Uuid::from(notification.reaction.by).into(),
                ],
            ))
            .await
            .map_err(map_db_err)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let model = user_notification::Model::from_query_result(&row, "").map_err(map_db_err)?;
        let replaced_unread_at: Option<DateTimeWithTimeZone> =
            row.try_get("", "replaced_unread_at").map_err(map_db_err)?;
        Ok(Some(StoredReactionNotification {
            notification: model.into(),
            replaced_unread_at: replaced_unread_at.map(Into::into),
        }))
    }

    async fn list_notifications(
        &self,
        params: ListUserNotificationsParams,
    ) -> RepoResult<Page<UserNotification, UserNotificationCursor>> {
        let mut q = user_notification::Entity::find()
            .filter(user_notification::Column::UserId.eq(Uuid::from(params.user_id)))
            .filter(user_notification::Column::ServerId.eq(Uuid::from(params.server_id)))
            .filter(not_hidden(params.user_id));
        if params.unread_only {
            q = q.filter(user_notification::Column::ReadAt.is_null());
        }

        // Newest first (DESC endpoint): `After` = older than the cursor, `Before` = newer.
        let has_cursor = params.cursor.is_some();
        if let Some(cursor) = params.cursor {
            let cursor_id: Uuid = cursor.notification_id.into();
            let cursor_ts: DateTimeWithTimeZone = cursor.created_at.into();
            let cond = match params.dir {
                PageDir::After => Condition::any()
                    .add(user_notification::Column::CreatedAt.lt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(user_notification::Column::CreatedAt.eq(cursor_ts))
                            .add(user_notification::Column::Id.lt(cursor_id)),
                    ),
                PageDir::Before => Condition::any()
                    .add(user_notification::Column::CreatedAt.gt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(user_notification::Column::CreatedAt.eq(cursor_ts))
                            .add(user_notification::Column::Id.gt(cursor_id)),
                    ),
            };
            q = q.filter(cond);
        }

        // `Before` pages are fetched nearest-first (ascending) and flipped back.
        let fetch_reversed = has_cursor && params.dir == PageDir::Before;
        q = if fetch_reversed {
            q.order_by_asc(user_notification::Column::CreatedAt)
                .order_by_asc(user_notification::Column::Id)
        } else {
            q.order_by_desc(user_notification::Column::CreatedAt)
                .order_by_desc(user_notification::Column::Id)
        };

        let limit = params.limit.max(1);
        let mut items: Vec<UserNotification> = q
            .limit(u64::from(limit) + 1)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(Into::into)
            .collect();

        let has_more_in_dir = items.len() > limit as usize;
        if has_more_in_dir {
            items.truncate(limit as usize);
        }
        if fetch_reversed {
            items.reverse();
        }

        let to_cursor = |n: &UserNotification| UserNotificationCursor {
            created_at: n.created_at,
            notification_id: n.id,
        };
        // Items are newest first: `first` is the max, `last` the min.
        let max = items.first().map(to_cursor);
        let min = items.last().map(to_cursor);
        let has_opposite = has_cursor && !items.is_empty();
        let (next, prev) = match params.dir {
            PageDir::After => (
                has_more_in_dir.then_some(min).flatten(),
                has_opposite.then_some(max).flatten(),
            ),
            PageDir::Before => (
                has_more_in_dir.then_some(max).flatten(),
                has_opposite.then_some(min).flatten(),
            ),
        };

        Ok(Page::new(items, next, prev))
    }

    async fn unread_count(&self, user_id: UserId, server_id: ServerId) -> RepoResult<u64> {
        user_notification::Entity::find()
            .filter(user_notification::Column::UserId.eq(Uuid::from(user_id)))
            .filter(user_notification::Column::ServerId.eq(Uuid::from(server_id)))
            .filter(user_notification::Column::ReadAt.is_null())
            .filter(not_hidden(user_id))
            .count(&self.db)
            .await
            .map_err(map_db_err)
    }

    async fn mark_read_by_ids(
        &self,
        user_id: UserId,
        server_id: ServerId,
        ids: &[UserNotificationId],
        read_at: DateTime<Utc>,
    ) -> RepoResult<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let read_at: DateTimeWithTimeZone = read_at.into();
        let res = user_notification::Entity::update_many()
            .col_expr(
                user_notification::Column::ReadAt,
                sea_orm::sea_query::Expr::value(read_at),
            )
            .filter(user_notification::Column::UserId.eq(Uuid::from(user_id)))
            .filter(user_notification::Column::ServerId.eq(Uuid::from(server_id)))
            .filter(user_notification::Column::Id.is_in(uuids(ids)))
            .filter(user_notification::Column::ReadAt.is_null())
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }

    async fn mark_read_before(
        &self,
        user_id: UserId,
        server_id: ServerId,
        before: DateTime<Utc>,
        read_at: DateTime<Utc>,
    ) -> RepoResult<u64> {
        let read_at: DateTimeWithTimeZone = read_at.into();
        let before: DateTimeWithTimeZone = before.into();
        let res = user_notification::Entity::update_many()
            .col_expr(
                user_notification::Column::ReadAt,
                sea_orm::sea_query::Expr::value(read_at),
            )
            .filter(user_notification::Column::UserId.eq(Uuid::from(user_id)))
            .filter(user_notification::Column::ServerId.eq(Uuid::from(server_id)))
            .filter(user_notification::Column::CreatedAt.lte(before))
            .filter(user_notification::Column::ReadAt.is_null())
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }

    async fn mark_read_for_conversation(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
        up_to: DateTime<Utc>,
        read_at: DateTime<Utc>,
    ) -> RepoResult<u64> {
        let user_uuid: Uuid = user_id.into();
        let conv_uuid: Uuid = conversation_id.into();
        let up_to: DateTimeWithTimeZone = up_to.into();
        let read_at: DateTimeWithTimeZone = read_at.into();
        let res = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"UPDATE user_notification un
                   SET read_at = $4
                   FROM message m
                   WHERE un.user_id = $1
                     AND un.conversation_id = $2
                     AND un.read_at IS NULL
                     AND m.id = un.message_id
                     AND m.sent_at <= $3"#,
                [
                    user_uuid.into(),
                    conv_uuid.into(),
                    up_to.into(),
                    read_at.into(),
                ],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected())
    }

    async fn delete_unread_for_message(&self, message_id: MessageId) -> RepoResult<u64> {
        let res = user_notification::Entity::delete_many()
            .filter(user_notification::Column::MessageId.eq(Uuid::from(message_id)))
            .filter(user_notification::Column::ReadAt.is_null())
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }

    async fn delete_for_user_in_server(
        &self,
        user_id: UserId,
        server_id: ServerId,
    ) -> RepoResult<u64> {
        let res = user_notification::Entity::delete_many()
            .filter(user_notification::Column::UserId.eq(Uuid::from(user_id)))
            .filter(user_notification::Column::ServerId.eq(Uuid::from(server_id)))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }

    async fn delete_for_user_in_conversation(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
    ) -> RepoResult<u64> {
        let res = user_notification::Entity::delete_many()
            .filter(user_notification::Column::UserId.eq(Uuid::from(user_id)))
            .filter(user_notification::Column::ConversationId.eq(Uuid::from(conversation_id)))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }

    async fn delete_created_before(&self, cutoff: DateTime<Utc>) -> RepoResult<u64> {
        let cutoff: DateTimeWithTimeZone = cutoff.into();
        let res = user_notification::Entity::delete_many()
            .filter(user_notification::Column::CreatedAt.lt(cutoff))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }

    async fn list_digest_candidates(
        &self,
        hourly_cutoff: DateTime<Utc>,
        daily_cutoff: DateTime<Utc>,
    ) -> RepoResult<Vec<DigestCandidate>> {
        let hourly_cutoff: DateTimeWithTimeZone = hourly_cutoff.into();
        let daily_cutoff: DateTimeWithTimeZone = daily_cutoff.into();
        // `pending` aggregates unread, un-emailed rows; `last` is the newest `emailed_at` (the
        // previous digest) per user and workspace. The cutoff is the frequency's interval.
        // Users without an email address are never candidates. Reaction
        // notifications never count: digests list messages only.
        // Rows of conversations hidden by the request inbox wait until they are accepted.
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!(
                    r#"WITH pending AS (
                       SELECT user_id, server_id,
                              MIN(created_at) AS oldest_pending_at,
                              COUNT(*) AS pending_count
                       FROM user_notification
                       WHERE read_at IS NULL AND emailed_at IS NULL
                         AND reason <> 'reaction'
                         AND NOT {}
                       GROUP BY user_id, server_id
                   ),
                   last AS (
                       SELECT user_id, server_id, MAX(emailed_at) AS last_digest_at
                       FROM user_notification
                       WHERE emailed_at IS NOT NULL
                       GROUP BY user_id, server_id
                   )
                   SELECT s.user_id, s.server_id, u.email, srv.name AS server_name,
                          s.digest::text AS digest, p.oldest_pending_at, p.pending_count,
                          l.last_digest_at
                   FROM user_notification_settings s
                   JOIN pending p ON p.user_id = s.user_id AND p.server_id = s.server_id
                   JOIN "user" u ON u.id = s.user_id
                   JOIN server srv ON srv.id = s.server_id
                   LEFT JOIN last l ON l.user_id = s.user_id AND l.server_id = s.server_id
                   WHERE s.digest <> 'off'
                     AND u.email IS NOT NULL
                     AND p.oldest_pending_at <= CASE s.digest WHEN 'hourly' THEN $1 ELSE $2 END
                     AND (l.last_digest_at IS NULL
                          OR l.last_digest_at <= CASE s.digest WHEN 'hourly' THEN $1 ELSE $2 END)
                   ORDER BY p.oldest_pending_at, s.user_id, s.server_id"#,
                    hidden(
                        "user_notification.user_id",
                        "user_notification.conversation_id"
                    )
                ),
                [hourly_cutoff.into(), daily_cutoff.into()],
            ))
            .await
            .map_err(map_db_err)?;

        rows.into_iter()
            .map(|row| {
                let user_id: Uuid = row.try_get("", "user_id").map_err(map_db_err)?;
                let server_id: Uuid = row.try_get("", "server_id").map_err(map_db_err)?;
                let email: String = row.try_get("", "email").map_err(map_db_err)?;
                let server_name: String = row.try_get("", "server_name").map_err(map_db_err)?;
                let digest: String = row.try_get("", "digest").map_err(map_db_err)?;
                let oldest: DateTimeWithTimeZone =
                    row.try_get("", "oldest_pending_at").map_err(map_db_err)?;
                let pending_count: i64 = row.try_get("", "pending_count").map_err(map_db_err)?;
                let last: Option<DateTimeWithTimeZone> =
                    row.try_get("", "last_digest_at").map_err(map_db_err)?;
                let frequency = digest.parse::<DigestFrequency>().map_err(|e| {
                    RepoError::with_message(RepoErrorKind::InvalidData, e.to_string())
                })?;
                Ok(DigestCandidate {
                    user_id: user_id.into(),
                    server_id: server_id.into(),
                    email,
                    server_name,
                    frequency,
                    oldest_pending_at: oldest.into(),
                    pending_count: u64::try_from(pending_count).unwrap_or(0),
                    last_digest_at: last.map(Into::into),
                })
            })
            .collect()
    }

    async fn list_pending_digest(
        &self,
        user_id: UserId,
        server_id: ServerId,
        limit: u32,
    ) -> RepoResult<Vec<UserNotification>> {
        let models = user_notification::Entity::find()
            .filter(user_notification::Column::UserId.eq(Uuid::from(user_id)))
            .filter(user_notification::Column::ServerId.eq(Uuid::from(server_id)))
            .filter(user_notification::Column::ReadAt.is_null())
            .filter(user_notification::Column::EmailedAt.is_null())
            .filter(
                user_notification::Column::Reason
                    .ne(entity::sea_orm_active_enums::NotificationReason::Reaction),
            )
            .filter(not_hidden(user_id))
            .order_by_asc(user_notification::Column::CreatedAt)
            .order_by_asc(user_notification::Column::Id)
            .limit(u64::from(limit))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn mark_emailed(
        &self,
        ids: &[UserNotificationId],
        emailed_at: DateTime<Utc>,
    ) -> RepoResult<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let emailed_at: DateTimeWithTimeZone = emailed_at.into();
        let res = user_notification::Entity::update_many()
            .col_expr(
                user_notification::Column::EmailedAt,
                sea_orm::sea_query::Expr::value(emailed_at),
            )
            .filter(user_notification::Column::Id.is_in(uuids(ids)))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{DbBackend, EntityTrait, QueryFilter, QueryTrait};

    use super::*;

    #[test]
    fn not_hidden_binds_the_user_to_its_own_placeholders() {
        let server = Uuid::now_v7();
        let user = UserId::new();
        let stmt = user_notification::Entity::find()
            .filter(user_notification::Column::ServerId.eq(server))
            .filter(not_hidden(user))
            .build(DbBackend::Postgres);
        let values = stmt.values.expect("values").0;
        let user = sea_orm::Value::from(Uuid::from(user));
        assert_eq!(values.len(), 3);
        assert_eq!(values[1], user);
        assert_eq!(values[2], user);
        assert!(stmt.sql.contains("r.user_id = $2"), "{}", stmt.sql);
        assert!(stmt.sql.contains("b.user_id = $3"), "{}", stmt.sql);
        assert!(
            stmt.sql
                .contains(r#"r.conversation_id = "user_notification"."conversation_id""#),
            "{}",
            stmt.sql
        );
    }
}
