use async_trait::async_trait;
use entity::message;
use rim_core::message::model::{Message, MessageId, NewMessage};
use rim_core::message::repository::{
    ListMessagesParams, MessageCursor, MessageRepository, MessageSearchHit, MessageSearchParams,
};
use rim_core::repository::error::RepoResult;
use rim_core::repository::error::{RepoError, RepoErrorKind};
use rim_core::repository::pagination::{Page, PageDir, SortOrder};
use sea_orm::QuerySelect;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DbBackend, EntityTrait, FromQueryResult, QueryFilter,
    QueryOrder, Set, Statement, Value,
};
use std::collections::HashMap;

use crate::DbHandle;
use crate::error::map_db_err;

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
    sent_by: Uuid,
    text: Option<String>,
    is_deleted: bool,
    has_attachments: bool,
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
                sent_by: row.sent_by.into(),
                text: row.text,
                is_deleted: row.is_deleted,
                has_attachments: row.has_attachments,
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
            has_attachments: Set(!new_message.attachments.is_empty()),
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
        // it and the cursor (finding A-H1).
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
        // would return the far end of the range and skip the rows next to the cursor (A-H1).
        let fetch_reversed = cursor.is_some() && dir == PageDir::Before;
        let fetch_dir = if fetch_reversed { "ASC" } else { "DESC" };

        let sql = format!(
            r#"SELECT m.id, m.conversation_id, m.thread_id, m.parent_message_id,
                      m.forwarded_from_message_id, m.sent_at, m.sent_by, m.text, m.is_deleted,
                      m.has_attachments,
                      ts_rank(m.search_tsv, q.query) AS rank,
                      ts_headline('english', coalesce(m.text, ''), q.query,
                                  'StartSel=<mark>,StopSel=</mark>,MaxFragments=1') AS headline
               FROM message m,
                    websearch_to_tsquery('english', $1) AS q(query)
               WHERE m.conversation_id = ANY($2)
                 AND m.is_deleted = false
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
        active.update(&self.db).await.map_err(map_db_err)?;

        Ok(())
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
}
