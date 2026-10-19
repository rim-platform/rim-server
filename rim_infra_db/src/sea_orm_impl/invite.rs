use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::invite;
use sea_orm::Value;
use sea_orm::prelude::{DateTimeWithTimeZone, Expr, Uuid};
use sea_orm::{
    ColumnTrait, Condition, EntityTrait, ExprTrait, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};

use crate::DbHandle;
use crate::error::map_db_err;
use rim_core::auth::model::{Invite, InviteId, NewInvite};
use rim_core::auth::repository::{InviteCursor, InviteRepository, ListInvitesParams};
use rim_core::repository::error::RepoResult;
use rim_core::repository::pagination::{Page, PageDir};
use rim_core::user::model::UserId;

/// SeaORM-backed implementation of [`InviteRepository`] (workspace invites).
pub struct SeaOrmInviteRepository {
    db: DbHandle,
}

impl SeaOrmInviteRepository {
    /// Construct a new repository using the given [`DbHandle`].
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl InviteRepository for SeaOrmInviteRepository {
    async fn get_by_id(&self, id: InviteId) -> RepoResult<Option<Invite>> {
        let model = invite::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Invite>> {
        let model = invite::Entity::find()
            .filter(invite::Column::TokenHash.eq(token_hash))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn create_invite(&self, inv: NewInvite) -> RepoResult<Invite> {
        let active_model = invite::ActiveModel {
            id: Set(InviteId::new().into()),
            server_id: Set(inv.server_id.into()),
            created_by_user_id: Set(inv.created_by_user_id.into()),
            token_hash: Set(inv.token_hash),
            expires_at: Set(inv.expires_at.map(Into::into)),
            max_uses: Set(inv.max_uses),
            role: Set(inv.role.into()),
            ..Default::default()
        };

        let res = invite::Entity::insert(active_model)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.into())
    }

    async fn try_consume(&self, id: InviteId) -> RepoResult<bool> {
        // One update checks the uses, the revocation and the expiry, so a revoke or an expiry
        // between the caller's check and this call refuses the use.
        let now = Value::ChronoDateTimeUtc(Some(Utc::now()));
        let res = invite::Entity::update_many()
            .filter(invite::Column::Id.eq::<Uuid>(id.into()))
            .filter(Expr::col(invite::Column::UseCount).lt(Expr::col(invite::Column::MaxUses)))
            .filter(invite::Column::RevokedAt.is_null())
            .filter(
                Condition::any()
                    .add(invite::Column::ExpiresAt.is_null())
                    .add(invite::Column::ExpiresAt.gt(now)),
            )
            .col_expr(
                invite::Column::UseCount,
                Expr::col(invite::Column::UseCount).add(1),
            )
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(res.rows_affected == 1)
    }

    async fn revoke(&self, id: InviteId, at: DateTime<Utc>) -> RepoResult<()> {
        invite::Entity::update_many()
            .filter(invite::Column::Id.eq::<Uuid>(id.into()))
            .filter(invite::Column::RevokedAt.is_null())
            .col_expr(
                invite::Column::RevokedAt,
                Expr::value(Value::ChronoDateTimeUtc(Some(at))),
            )
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn list_for_server(
        &self,
        params: ListInvitesParams,
    ) -> RepoResult<Page<Invite, InviteCursor>> {
        let mut q = invite::Entity::find()
            .filter(invite::Column::ServerId.eq(Uuid::from(params.server_id)));

        // Newest first (DESC endpoint): `After` = older than the cursor, `Before` = newer.
        let has_cursor = params.cursor.is_some();
        if let Some(cursor) = params.cursor {
            let cursor_id: Uuid = cursor.invite_id.into();
            let cursor_ts: DateTimeWithTimeZone = cursor.created_at.into();
            let cond = match params.dir {
                PageDir::After => Condition::any()
                    .add(invite::Column::CreatedAt.lt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(invite::Column::CreatedAt.eq(cursor_ts))
                            .add(invite::Column::Id.lt(cursor_id)),
                    ),
                PageDir::Before => Condition::any()
                    .add(invite::Column::CreatedAt.gt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(invite::Column::CreatedAt.eq(cursor_ts))
                            .add(invite::Column::Id.gt(cursor_id)),
                    ),
            };
            q = q.filter(cond);
        }

        // `Before` pages are fetched nearest-first (ascending) and flipped back.
        let fetch_reversed = has_cursor && params.dir == PageDir::Before;
        q = if fetch_reversed {
            q.order_by_asc(invite::Column::CreatedAt)
                .order_by_asc(invite::Column::Id)
        } else {
            q.order_by_desc(invite::Column::CreatedAt)
                .order_by_desc(invite::Column::Id)
        };

        let limit = Ord::max(params.limit, 1);
        let mut items: Vec<Invite> = q
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

        let to_cursor = |i: &Invite| InviteCursor {
            created_at: i.created_at,
            invite_id: i.id,
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

    async fn count_created_since(
        &self,
        user_id: UserId,
        since: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<u64> {
        let since: sea_orm::prelude::DateTimeWithTimeZone = since.into();
        invite::Entity::find()
            .filter(invite::Column::CreatedByUserId.eq::<Uuid>(user_id.into()))
            .filter(invite::Column::CreatedAt.gte(since))
            .count(&self.db)
            .await
            .map_err(map_db_err)
    }
}
