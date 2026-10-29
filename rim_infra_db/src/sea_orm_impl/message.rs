use async_trait::async_trait;
use entity::message;
use rim_core::message::model::{Message, MessageId, NewMessage};
use rim_core::message::repository::{
    AuthorChannelCounts, AuthorChannelScope, DeletedMessage, ListMessagesParams, MessageChange,
    MessageCursor, MessageRepository, MessageSearchHit, MessageSearchParams, MessageTally,
};
use rim_core::repository::error::RepoResult;
use rim_core::repository::error::{RepoError, RepoErrorKind};
use rim_core::repository::pagination::{Page, PageDir, SortOrder};
use sea_orm::QuerySelect;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DbBackend, EntityTrait,
    FromQueryResult, QueryFilter, QueryOrder, Set, Statement, Value,
};
use std::collections::HashMap;

use crate::DbHandle;
use crate::error::map_db_err;

/// Messages up to this many characters come back whole as their search snippet.
const WHOLE_HEADLINE_MAX_CHARS: usize = 300;

/// One author's undeleted, unencrypted messages in a workspace's channels that are neither
/// archived nor encrypted, each with `unjoined` true in a private channel the viewer is not a
/// (non-pending) member of. Binds `$1` the workspace, `$2` the author, `$3` the inclusive lower
/// bound on `sent_at` (null for every message) and `$4` the viewer.
const AUTHOR_CHANNEL_MESSAGES_SQL: &str = r#"
SELECT m.conversation_id, m.sent_at,
       (c.is_private AND NOT EXISTS (
           SELECT 1 FROM conversation_member cm
           WHERE cm.conversation_id = c.id AND cm.user_id = $4 AND cm.pending_since IS NULL)) AS unjoined
FROM message m JOIN conversation c ON c.id = m.conversation_id
WHERE c.server_id = $1 AND c.conversation_type = 'channel'
  AND c.archived_at IS NULL AND c.encrypted_at IS NULL
  AND m.sent_by = $2 AND m.is_deleted = false AND m.encrypted = false
  AND ($3::timestamptz IS NULL OR m.sent_at >= $3)
"#;

/// The four binds of [`AUTHOR_CHANNEL_MESSAGES_SQL`].
fn author_scope_values(scope: AuthorChannelScope) -> Vec<Value> {
    vec![
        Value::Uuid(Some(Uuid::from(scope.server_id))),
        Value::Uuid(Some(Uuid::from(scope.author))),
        Value::ChronoDateTimeUtc(scope.since),
        Value::Uuid(Some(Uuid::from(scope.viewer))),
    ]
}

/// Reads a non-negative SQL count as `u64`.
fn count_column(row: &sea_orm::QueryResult, column: &str) -> RepoResult<u64> {
    let n = row.try_get::<i64>("", column).map_err(map_db_err)?;
    Ok(u64::try_from(n).unwrap_or(0))
}

pub struct SeaOrmMessageRepository {
    db: DbHandle,
}

impl SeaOrmMessageRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

/// Row shape for the full-text-search query in [`SeaOrmMessageRepository::search_messages_fts`].
/// Field names must match the query's column/alias names exactly (`sea_orm::FromQueryResult`
/// binds by name). Mirrors `message::Model`'s columns (selected explicitly rather than `m.*`, so
/// the field list stays a single source of truth) plus the two computed FTS columns.
#[derive(Debug, FromQueryResult)]
struct MessageSearchRow {
    id: Uuid,
    conversation_id: Uuid,
    thread_id: Option<Uuid>,
    parent_message_id: Option<Uuid>,
    forwarded_from_message_id: Option<Uuid>,
    sent_at: DateTimeWithTimeZone,
    edited_at: Option<DateTimeWithTimeZone>,
    sent_by: Uuid,
    text: Option<String>,
    is_deleted: bool,
    has_attachments: bool,
    flagged_at: Option<DateTimeWithTimeZone>,
    rank: f32,
    headline: Option<String>,
}

impl From<MessageSearchRow> for MessageSearchHit {
    fn from(row: MessageSearchRow) -> Self {
        MessageSearchHit {
            message: Message {
                id: row.id.into(),
                conversation_id: row.conversation_id.into(),
                thread_id: row.thread_id.map(Into::into),
                parent_message_id: row.parent_message_id.map(Into::into),
                forwarded_from_message_id: row.forwarded_from_message_id.map(Into::into),
                sent_at: row.sent_at.into(),
                edited_at: row.edited_at.map(Into::into),
                sent_by: row.sent_by.into(),
                text: row.text,
                is_deleted: row.is_deleted,
                has_attachments: row.has_attachments,
                encryption: None,
                flagged_at: row.flagged_at.map(Into::into),
            },
            rank: row.rank,
            headline: row.headline,
        }
    }
}

#[async_trait]
impl MessageRepository for SeaOrmMessageRepository {
    async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>> {
        let model = message::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn get_by_ids(&self, ids: &[MessageId]) -> RepoResult<HashMap<MessageId, Message>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }

        let ids = ids.iter().copied().map(Into::into).collect::<Vec<Uuid>>();
        let models = message::Entity::find()
            .filter(message::Column::Id.is_in(ids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models
            .into_iter()
            .map(Into::<Message>::into)
            .map(|m| (m.id, m))
            .collect())
    }

    async fn create_message(&self, new_message: &NewMessage) -> RepoResult<Message> {
        let active = message::ActiveModel {
            id: Set(MessageId::new().into()),
            conversation_id: Set(new_message.conversation_id.into()),
            thread_id: Set(new_message.thread_id.map(Into::into)),
            parent_message_id: Set(new_message.parent_message_id.map(Into::into)),
            forwarded_from_message_id: Set(new_message.forwarded_from_message_id.map(Into::into)),
            sent_by: Set(new_message.sent_by.into()),
            text: Set(new_message.text.to_owned()),
            is_deleted: Set(false),
            has_attachments: Set(false),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn list_messages(
        &self,
        params: ListMessagesParams,
    ) -> RepoResult<Page<Message, MessageCursor>> {
        let ListMessagesParams {
            conversation_id,
            thread_id,
            limit,
            dir,
            cursor,
            include_deleted,
            order,
        } = params;

        let mut q = message::Entity::find()
            .filter(message::Column::ConversationId.eq::<Uuid>(conversation_id.into()));

        q = match thread_id {
            Some(thread_id) => q.filter(message::Column::ThreadId.eq::<Uuid>(thread_id.into())),
            None => q.filter(message::Column::ThreadId.is_null()),
        };

        if !include_deleted {
            q = q.filter(message::Column::IsDeleted.eq(false));
        }

        let has_cursor = cursor.is_some();
        if let Some(cursor) = cursor {
            let cursor_id: Uuid = cursor.message_id.into();
            let cursor_ts: DateTimeWithTimeZone = cursor.sent_at.into();

            let cond = match (order, dir) {
                (SortOrder::Asc, PageDir::Before) | (SortOrder::Desc, PageDir::After) => {
                    Condition::any()
                        .add(message::Column::SentAt.lt(cursor_ts))
                        .add(
                            Condition::all()
                                .add(message::Column::SentAt.eq(cursor_ts))
                                .add(message::Column::Id.lt(cursor_id)),
                        )
                }
                (SortOrder::Asc, PageDir::After) | (SortOrder::Desc, PageDir::Before) => {
                    Condition::any()
                        .add(message::Column::SentAt.gt(cursor_ts))
                        .add(
                            Condition::all()
                                .add(message::Column::SentAt.eq(cursor_ts))
                                .add(message::Column::Id.gt(cursor_id)),
                        )
                }
            };

            q = q.filter(cond);
        }

        // A page holds the rows adjacent to the cursor. `Before` rows precede the cursor in
        // `order`, so fetch them nearest-first (reversed order) and flip the page back below;
        // fetching in `order` would return the far end of the range and skip everything between
        // it and the cursor.
        let fetch_reversed = has_cursor && dir == PageDir::Before;
        let fetch_order = match (order, fetch_reversed) {
            (SortOrder::Asc, false) | (SortOrder::Desc, true) => SortOrder::Asc,
            (SortOrder::Desc, false) | (SortOrder::Asc, true) => SortOrder::Desc,
        };
        q = match fetch_order {
            SortOrder::Asc => q
                .order_by_asc(message::Column::SentAt)
                .order_by_asc(message::Column::Id),
            SortOrder::Desc => q
                .order_by_desc(message::Column::SentAt)
                .order_by_desc(message::Column::Id),
        };

        let mut items = q
            .limit((limit + 1) as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<Message>>();

        let has_more_in_dir = items.len() > limit as usize;
        if has_more_in_dir {
            items.truncate(limit as usize);
        }
        if fetch_reversed {
            items.reverse();
        }

        let (min, max) = match order {
            SortOrder::Asc => (items.first(), items.last()),
            SortOrder::Desc => (items.last(), items.first()),
        };

        let min = min.map(|m| MessageCursor {
            sent_at: m.sent_at,
            message_id: m.id,
        });
        let max = max.map(|m| MessageCursor {
            sent_at: m.sent_at,
            message_id: m.id,
        });

        let has_opposite = has_cursor && !items.is_empty();
        let (next, prev) = match (order, dir) {
            (SortOrder::Asc, PageDir::After) => (
                has_more_in_dir.then_some(max).flatten(),
                has_opposite.then_some(min).flatten(),
            ),
            (SortOrder::Asc, PageDir::Before) => (
                has_more_in_dir.then_some(min).flatten(),
                has_opposite.then_some(max).flatten(),
            ),
            (SortOrder::Desc, PageDir::After) => (
                has_more_in_dir.then_some(min).flatten(),
                has_opposite.then_some(max).flatten(),
            ),
            (SortOrder::Desc, PageDir::Before) => (
                has_more_in_dir.then_some(max).flatten(),
                has_opposite.then_some(min).flatten(),
            ),
        };

        Ok(Page::new(items, next, prev))
    }

    async fn search_messages_fts(
        &self,
        params: MessageSearchParams,
    ) -> RepoResult<Vec<MessageSearchHit>> {
        let MessageSearchParams {
            conversation_ids,
            tsquery,
            author_id,
            has_link,
            has_attachment,
            after,
            before,
            limit,
            cursor,
            dir,
        } = params;

        // Empty accessible set => empty result. Never "search everything" — see the
        // visibility-agnostic contract on `MessageSearchParams`.
        if conversation_ids.is_empty() {
            return Ok(vec![]);
        }

        let conv_uuids: Vec<Uuid> = conversation_ids.iter().copied().map(Into::into).collect();

        let author_id_val = match author_id {
            Some(uid) => Value::Uuid(Some(uid.into())),
            None => Value::Uuid(None),
        };
        let after_val = match after {
            Some(ts) => Value::ChronoDateTimeUtc(Some(ts)),
            None => Value::ChronoDateTimeUtc(None),
        };
        let before_val = match before {
            Some(ts) => Value::ChronoDateTimeUtc(Some(ts)),
            None => Value::ChronoDateTimeUtc(None),
        };
        let (cursor_sent_at_val, cursor_id_val) = match cursor {
            Some(c) => (
                Value::ChronoDateTimeUtc(Some(c.sent_at)),
                Value::Uuid(Some(c.message_id.into())),
            ),
            None => (Value::ChronoDateTimeUtc(None), Value::Uuid(None)),
        };

        // Order is always (sent_at DESC, id DESC) — see MessageRepository::search_messages_fts
        // doc comment for why relevance is never the sort key. The cursor comparison direction
        // flips with `dir`, mirroring the (SortOrder::Desc, dir) branches in `list_messages`
        // above: After = "strictly older than cursor" (<), Before = "strictly newer" (>).
        let op = match dir {
            PageDir::After => "<",
            PageDir::Before => ">",
        };
        // Select the rows nearest the cursor: `Before` rows are newer than it, so fetch them
        // oldest-first and flip them back to (sent_at DESC, id DESC) below. Fetching newest-first
        // would return the far end of the range and skip the rows next to the cursor.
        let fetch_reversed = cursor.is_some() && dir == PageDir::Before;
        let fetch_dir = if fetch_reversed { "ASC" } else { "DESC" };

        let sql = format!(
            r#"SELECT m.id, m.conversation_id, m.thread_id, m.parent_message_id,
                      m.forwarded_from_message_id, m.sent_at, m.edited_at, m.sent_by, m.text,
                      m.is_deleted, m.has_attachments, m.flagged_at,
                      ts_rank(m.search_tsv, q.query) AS rank,
                      ts_headline('english', coalesce(m.text, ''), q.query,
                                  CASE WHEN char_length(coalesce(m.text, '')) <= {WHOLE_HEADLINE_MAX_CHARS}
                                       THEN 'StartSel=<mark>,StopSel=</mark>,HighlightAll=true'
                                       ELSE 'StartSel=<mark>,StopSel=</mark>,MaxFragments=1,ShortWord=0' END) AS headline
               FROM message m,
                    websearch_to_tsquery('english', $1) AS q(query)
               WHERE m.conversation_id = ANY($2)
                 AND m.is_deleted = false
                 AND NOT m.encrypted
                 AND ($1 = '' OR m.search_tsv @@ q.query)
                 AND ($3::uuid IS NULL OR m.sent_by = $3)
                 AND (NOT $4 OR m.text ~* 'https?://')
                 AND (NOT $5 OR m.has_attachments = true)
                 AND ($6::timestamptz IS NULL OR m.sent_at >= $6)
                 AND ($7::timestamptz IS NULL OR m.sent_at < $7)
                 AND ($8::timestamptz IS NULL
                      OR (m.sent_at {op} $8 OR (m.sent_at = $8 AND m.id {op} $9)))
               ORDER BY m.sent_at {fetch_dir}, m.id {fetch_dir}
               LIMIT $10"#
        );

        let rows = MessageSearchRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            &sql,
            [
                tsquery.into(),
                conv_uuids.into(),
                author_id_val,
                has_link.into(),
                has_attachment.into(),
                after_val,
                before_val,
                cursor_sent_at_val,
                cursor_id_val,
                Value::BigInt(Some(i64::from(limit) + 1)),
            ],
        ))
        .all(&self.db)
        .await
        .map_err(map_db_err)?;

        let mut hits: Vec<MessageSearchHit> = rows.into_iter().map(Into::into).collect();
        if fetch_reversed {
            hits.reverse();
        }
        Ok(hits)
    }

    async fn update_message_text(
        &self,
        message_id: MessageId,
        new_text: String,
    ) -> RepoResult<Message> {
        let msg = message::Entity::find_by_id(message_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: message::ActiveModel = msg.into();
        active.text = Set(Some(new_text));
        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn set_has_attachments(
        &self,
        message_id: MessageId,
        has_attachments: bool,
    ) -> RepoResult<Message> {
        let msg = message::Entity::find_by_id(message_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: message::ActiveModel = msg.into();
        active.has_attachments = Set(has_attachments);
        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn soft_delete_message(&self, message_id: MessageId) -> RepoResult<()> {
        let msg = message::Entity::find_by_id(message_id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        let Some(msg) = msg else {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        };

        let mut active: message::ActiveModel = msg.into();
        active.is_deleted = Set(true);
        // A deleted encrypted message keeps its franking columns, not its ciphertext.
        active.mls_ciphertext = Set(None);
        active.update(&self.db).await.map_err(map_db_err)?;

        Ok(())
    }

    async fn set_flagged_at(
        &self,
        message_id: MessageId,
        flagged_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> RepoResult<bool> {
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE message SET flagged_at = $2 \
                 WHERE id = $1 AND is_deleted = false \
                   AND (($2::timestamptz IS NULL) <> (flagged_at IS NULL))",
                [
                    Value::Uuid(Some(Uuid::from(message_id))),
                    Value::ChronoDateTimeUtc(flagged_at),
                ],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_message_changes(
        &self,
        conversation_id: rim_core::conversation::model::ConversationId,
        after_version: i64,
        limit: u32,
    ) -> RepoResult<Vec<MessageChange>> {
        let models = message::Entity::find()
            .filter(message::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .filter(message::Column::Version.gt(after_version))
            .order_by_asc(message::Column::Version)
            .limit(u64::from(limit) + 1)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models
            .into_iter()
            .map(|m| MessageChange {
                version: m.version,
                edited_at: m.edited_at.map(Into::into),
                message: m.into(),
            })
            .collect())
    }

    async fn latest_change_version(
        &self,
        conversation_id: rim_core::conversation::model::ConversationId,
    ) -> RepoResult<i64> {
        let conversation_uuid: Uuid = conversation_id.into();
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT last_version FROM message_change_seq WHERE conversation_id = $1",
                [conversation_uuid.into()],
            ))
            .await
            .map_err(map_db_err)?;
        row.map_or(Ok(0), |row| {
            row.try_get::<i64>("", "last_version").map_err(map_db_err)
        })
    }

    async fn last_sent_at_by_author(
        &self,
        conversation_id: rim_core::conversation::model::ConversationId,
        author: rim_core::user::model::UserId,
    ) -> RepoResult<Option<chrono::DateTime<chrono::Utc>>> {
        let latest: Option<DateTimeWithTimeZone> = message::Entity::find()
            .select_only()
            .column(message::Column::SentAt)
            .filter(message::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .filter(message::Column::SentBy.eq::<Uuid>(author.into()))
            .order_by_desc(message::Column::SentAt)
            .into_tuple()
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(latest.map(Into::into))
    }

    async fn list_thread_participant_ids(
        &self,
        thread_id: rim_core::message_thread::model::MessageThreadId,
    ) -> RepoResult<Vec<rim_core::user::model::UserId>> {
        let thread_uuid: Uuid = thread_id.into();
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"SELECT DISTINCT m.sent_by AS user_id
                   FROM message m
                   WHERE (m.thread_id = $1 AND m.is_deleted = false)
                      OR m.id = (SELECT t.first_message_id FROM message_thread t WHERE t.id = $1)"#,
                [thread_uuid.into()],
            ))
            .await
            .map_err(map_db_err)?;
        rows.into_iter()
            .map(|row| {
                row.try_get::<Uuid>("", "user_id")
                    .map(Into::into)
                    .map_err(map_db_err)
            })
            .collect()
    }

    async fn count_author_channel_messages(
        &self,
        scope: AuthorChannelScope,
    ) -> RepoResult<AuthorChannelCounts> {
        let sql = format!(
            "WITH scoped AS ({AUTHOR_CHANNEL_MESSAGES_SQL}) \
             SELECT count(*) FILTER (WHERE NOT unjoined) AS readable_messages, \
                    count(DISTINCT conversation_id) FILTER (WHERE NOT unjoined) AS readable_channels, \
                    count(*) FILTER (WHERE unjoined) AS unjoined_messages, \
                    count(DISTINCT conversation_id) FILTER (WHERE unjoined) AS unjoined_channels \
             FROM scoped"
        );
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                author_scope_values(scope),
            ))
            .await
            .map_err(map_db_err)?;
        let Some(row) = row else {
            return Ok(AuthorChannelCounts::default());
        };
        Ok(AuthorChannelCounts {
            readable: MessageTally {
                messages: count_column(&row, "readable_messages")?,
                channels: count_column(&row, "readable_channels")?,
            },
            unjoined_private: MessageTally {
                messages: count_column(&row, "unjoined_messages")?,
                channels: count_column(&row, "unjoined_channels")?,
            },
        })
    }

    async fn list_author_channels(
        &self,
        scope: AuthorChannelScope,
        include_unjoined_private: bool,
    ) -> RepoResult<Vec<rim_core::conversation::model::ConversationId>> {
        let sql = format!(
            "WITH scoped AS ({AUTHOR_CHANNEL_MESSAGES_SQL}) \
             SELECT conversation_id FROM scoped WHERE ($5 OR NOT unjoined) \
             GROUP BY conversation_id ORDER BY max(sent_at) DESC, conversation_id"
        );
        let mut values = author_scope_values(scope);
        values.push(Value::Bool(Some(include_unjoined_private)));
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .map_err(map_db_err)?;
        rows.into_iter()
            .map(|row| {
                row.try_get::<Uuid>("", "conversation_id")
                    .map(Into::into)
                    .map_err(map_db_err)
            })
            .collect()
    }

    async fn soft_delete_author_channel_messages(
        &self,
        conversation_id: rim_core::conversation::model::ConversationId,
        author: rim_core::user::model::UserId,
        since: Option<chrono::DateTime<chrono::Utc>>,
        limit: u32,
    ) -> RepoResult<Vec<DeletedMessage>> {
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"UPDATE message SET is_deleted = true
                   WHERE id IN (
                       SELECT id FROM message
                       WHERE conversation_id = $1 AND sent_by = $2 AND is_deleted = false
                         AND encrypted = false
                         AND ($3::timestamptz IS NULL OR sent_at >= $3)
                       ORDER BY sent_at DESC, id DESC LIMIT $4 FOR UPDATE)
                     AND is_deleted = false
                     AND EXISTS (SELECT 1 FROM conversation c
                                 WHERE c.id = $1 AND c.conversation_type = 'channel'
                                   AND c.archived_at IS NULL AND c.encrypted_at IS NULL)
                   RETURNING id, thread_id"#,
                [
                    Value::Uuid(Some(Uuid::from(conversation_id))),
                    Value::Uuid(Some(Uuid::from(author))),
                    Value::ChronoDateTimeUtc(since),
                    Value::BigInt(Some(i64::from(limit))),
                ],
            ))
            .await
            .map_err(map_db_err)?;
        rows.into_iter()
            .map(|row| {
                let id = row.try_get::<Uuid>("", "id").map_err(map_db_err)?;
                let thread_id = row
                    .try_get::<Option<Uuid>>("", "thread_id")
                    .map_err(map_db_err)?;
                Ok(DeletedMessage {
                    id: id.into(),
                    thread_id: thread_id.map(Into::into),
                })
            })
            .collect()
    }
}
