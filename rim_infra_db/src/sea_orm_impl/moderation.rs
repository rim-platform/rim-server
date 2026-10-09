use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::{message_report, server_ban, server_member_timeout};
use rim_core::message::model::MessageId;
use rim_core::moderation::model::{
    MemberTimeout, MessageReport, MessageReportId, MessageReportStatus, NewMessageReport, ServerBan,
};
use rim_core::moderation::repository::{MessageReportCursor, ModerationRepository};
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::repository::pagination::{Page, PageDir};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::sea_query::{Alias, Expr, ExprTrait, OnConflict};
use sea_orm::{
    ColumnTrait, Condition, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmModerationRepository {
    db: DbHandle,
}

impl SeaOrmModerationRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ModerationRepository for SeaOrmModerationRepository {
    async fn upsert_ban(&self, ban: ServerBan) -> RepoResult<ServerBan> {
        let active = server_ban::ActiveModel {
            server_id: Set(ban.server_id.into()),
            user_id: Set(ban.user_id.into()),
            reason: Set(ban.reason),
            banned_by: Set(ban.banned_by.map(Into::into)),
            created_at: Set(ban.created_at.into()),
            expires_at: Set(ban.expires_at.map(Into::into)),
        };
        let model = server_ban::Entity::insert(active)
            .on_conflict(
                OnConflict::columns([server_ban::Column::ServerId, server_ban::Column::UserId])
                    .update_columns([
                        server_ban::Column::Reason,
                        server_ban::Column::BannedBy,
                        server_ban::Column::CreatedAt,
                        server_ban::Column::ExpiresAt,
                    ])
                    .to_owned(),
            )
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.into())
    }

    async fn get_active_ban(
        &self,
        server_id: ServerId,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<Option<ServerBan>> {
        let now: DateTimeWithTimeZone = now.into();
        let model = server_ban::Entity::find()
            .filter(server_ban::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(server_ban::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(
                Condition::any()
                    .add(server_ban::Column::ExpiresAt.is_null())
                    .add(server_ban::Column::ExpiresAt.gt(now)),
            )
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn delete_ban(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool> {
        let res = server_ban::Entity::delete_many()
            .filter(server_ban::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(server_ban::Column::UserId.eq::<Uuid>(user_id.into()))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected > 0)
    }

    async fn list_active_bans(
        &self,
        server_id: ServerId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<ServerBan>> {
        let now: DateTimeWithTimeZone = now.into();
        let models = server_ban::Entity::find()
            .filter(server_ban::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(
                Condition::any()
                    .add(server_ban::Column::ExpiresAt.is_null())
                    .add(server_ban::Column::ExpiresAt.gt(now)),
            )
            .order_by_desc(server_ban::Column::CreatedAt)
            .order_by_desc(server_ban::Column::UserId)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn upsert_timeout(&self, timeout: MemberTimeout) -> RepoResult<MemberTimeout> {
        let active = server_member_timeout::ActiveModel {
            server_id: Set(timeout.server_id.into()),
            user_id: Set(timeout.user_id.into()),
            until: Set(timeout.until.into()),
            reason: Set(timeout.reason),
            set_by: Set(timeout.set_by.map(Into::into)),
            created_at: Set(timeout.created_at.into()),
        };
        let model = server_member_timeout::Entity::insert(active)
            .on_conflict(
                OnConflict::columns([
                    server_member_timeout::Column::ServerId,
                    server_member_timeout::Column::UserId,
                ])
                .update_columns([
                    server_member_timeout::Column::Until,
                    server_member_timeout::Column::Reason,
                    server_member_timeout::Column::SetBy,
                    server_member_timeout::Column::CreatedAt,
                ])
                .to_owned(),
            )
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.into())
    }

    async fn get_active_timeout(
        &self,
        server_id: ServerId,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<Option<MemberTimeout>> {
        let now: DateTimeWithTimeZone = now.into();
        let model = server_member_timeout::Entity::find()
            .filter(server_member_timeout::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(server_member_timeout::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(server_member_timeout::Column::Until.gt(now))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn delete_timeout(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool> {
        let res = server_member_timeout::Entity::delete_many()
            .filter(server_member_timeout::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(server_member_timeout::Column::UserId.eq::<Uuid>(user_id.into()))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected > 0)
    }

    async fn list_active_timeouts(
        &self,
        server_id: ServerId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<MemberTimeout>> {
        let now: DateTimeWithTimeZone = now.into();
        let models = server_member_timeout::Entity::find()
            .filter(server_member_timeout::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(server_member_timeout::Column::Until.gt(now))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn create_report(&self, report: NewMessageReport) -> RepoResult<MessageReport> {
        let active = message_report::ActiveModel {
            id: Set(MessageReportId::new().into()),
            server_id: Set(report.server_id.into()),
            conversation_id: Set(report.conversation_id.into()),
            message_id: Set(report.message_id.into()),
            reporter_user_id: Set(report.reporter_user_id.into()),
            reason: Set(report.reason),
            franking_verified_at: Set(report.franked.as_ref().map(|_| Utc::now().into())),
            franked_sender_device_id: Set(report.franked.as_ref().map(|f| f.sender_device_id)),
            franked_content: Set(report.franked.map(|f| f.content)),
            status: Set(MessageReportStatus::Open.into()),
            resolved_by: Set(None),
            resolved_at: Set(None),
            created_at: Set(Utc::now().into()),
        };
        let model = message_report::Entity::insert(active)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.into())
    }

    async fn get_report(&self, id: MessageReportId) -> RepoResult<Option<MessageReport>> {
        let model = message_report::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn has_verified_report(
        &self,
        server_id: ServerId,
        message_id: MessageId,
    ) -> RepoResult<bool> {
        let count = message_report::Entity::find()
            .filter(message_report::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(message_report::Column::MessageId.eq::<Uuid>(message_id.into()))
            .filter(message_report::Column::FrankingVerifiedAt.is_not_null())
            .count(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(count > 0)
    }

    async fn list_reports(
        &self,
        server_id: ServerId,
        status: Option<MessageReportStatus>,
        limit: u32,
        dir: PageDir,
        cursor: Option<MessageReportCursor>,
    ) -> RepoResult<Page<MessageReport, MessageReportCursor>> {
        let mut q = message_report::Entity::find()
            .filter(message_report::Column::ServerId.eq::<Uuid>(server_id.into()));
        if let Some(status) = status {
            q = q.filter(message_report::Column::Status.eq(
                entity::sea_orm_active_enums::MessageReportStatus::from(status),
            ));
        }

        let has_cursor = cursor.is_some();
        if let Some(cursor) = cursor {
            let cursor_id: Uuid = cursor.report_id.into();
            let cursor_ts: DateTimeWithTimeZone = cursor.created_at.into();
            // DESC endpoint (docs/pagination.md): `Before` is newer than the cursor, `After`
            // is older.
            let cond = match dir {
                PageDir::Before => Condition::any()
                    .add(message_report::Column::CreatedAt.gt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(message_report::Column::CreatedAt.eq(cursor_ts))
                            .add(message_report::Column::Id.gt(cursor_id)),
                    ),
                PageDir::After => Condition::any()
                    .add(message_report::Column::CreatedAt.lt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(message_report::Column::CreatedAt.eq(cursor_ts))
                            .add(message_report::Column::Id.lt(cursor_id)),
                    ),
            };
            q = q.filter(cond);
        }

        // Newest first. `Before` pages are fetched nearest-first (ascending) and flipped
        // back, as in `SeaOrmMessageRepository::list_messages`.
        let fetch_reversed = has_cursor && dir == PageDir::Before;
        q = if fetch_reversed {
            q.order_by_asc(message_report::Column::CreatedAt)
                .order_by_asc(message_report::Column::Id)
        } else {
            q.order_by_desc(message_report::Column::CreatedAt)
                .order_by_desc(message_report::Column::Id)
        };

        let mut items: Vec<MessageReport> = q
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

        let to_cursor = |r: &MessageReport| MessageReportCursor {
            created_at: r.created_at,
            report_id: r.id,
        };
        // Items are newest-first, so `first` is the newest row and `last` the oldest.
        let newest = items.first().map(to_cursor);
        let oldest = items.last().map(to_cursor);
        let has_opposite = has_cursor && !items.is_empty();
        let (next, prev) = match dir {
            PageDir::After => (
                has_more_in_dir.then_some(oldest).flatten(),
                has_opposite.then_some(newest).flatten(),
            ),
            PageDir::Before => (
                has_more_in_dir.then_some(newest).flatten(),
                has_opposite.then_some(oldest).flatten(),
            ),
        };

        Ok(Page::new(items, next, prev))
    }

    async fn resolve_report(
        &self,
        id: MessageReportId,
        status: MessageReportStatus,
        resolved_by: UserId,
        resolved_at: DateTime<Utc>,
    ) -> RepoResult<MessageReport> {
        let id_uuid: Uuid = id.into();
        // Conditional update: only an open report moves, so two moderators resolving the same
        // report concurrently cannot both succeed.
        let res = message_report::Entity::update_many()
            .col_expr(
                message_report::Column::Status,
                // A bare text value does not coerce to the Postgres enum in an UPDATE ... SET.
                Expr::val(status.as_str()).as_enum(Alias::new("message_report_status")),
            )
            .col_expr(
                message_report::Column::ResolvedBy,
                Expr::val(Uuid::from(resolved_by)),
            )
            .col_expr(
                message_report::Column::ResolvedAt,
                Expr::val(DateTimeWithTimeZone::from(resolved_at)),
            )
            .filter(message_report::Column::Id.eq(id_uuid))
            .filter(
                message_report::Column::Status
                    .eq(entity::sea_orm_active_enums::MessageReportStatus::Open),
            )
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        let model = message_report::Entity::find_by_id(id_uuid)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
        if res.rows_affected == 0 {
            return Err(RepoError::new(RepoErrorKind::PreconditionFailed));
        }
        Ok(model.into())
    }
}
