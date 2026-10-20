use async_trait::async_trait;
use entity::user;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::{
    CreatedInvite, InviteChain, InviteKind, NewUser, UpdateProfilePatch, User, UserId,
};
use rim_core::user::repository::UserRepository;
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, FromQueryResult,
    IntoActiveModel, QueryFilter, QueryOrder, Set, Statement, Value, sea_query::Expr,
};

use crate::DbHandle;
use crate::error::map_db_err;
use crate::sea_orm_impl::user_lock::lock_user_row;
use rim_core::repository::error::{RepoError, RepoErrorKind};

fn statement(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

#[derive(FromQueryResult)]
struct FlagRow {
    flag: bool,
}

#[derive(FromQueryResult)]
struct IdRow {
    id: Uuid,
}

#[derive(FromQueryResult)]
struct CreatedInviteRow {
    id: Uuid,
    kind: String,
    created_at: DateTimeWithTimeZone,
    expires_at: Option<DateTimeWithTimeZone>,
    max_uses: i32,
    use_count: i32,
    revoked_at: Option<DateTimeWithTimeZone>,
}

pub struct SeaOrmUserRepository {
    pub db: DbHandle,
}

impl SeaOrmUserRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl UserRepository for SeaOrmUserRepository {
    async fn get_by_id(&self, id: UserId) -> RepoResult<Option<User>> {
        let model = user::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn get_by_ids(&self, ids: &[UserId]) -> RepoResult<Vec<User>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let uuids: Vec<sea_orm::prelude::Uuid> = ids.iter().copied().map(Into::into).collect();
        let models = user::Entity::find()
            .filter(user::Column::Id.is_in(uuids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn lock_user(&self, id: UserId) -> RepoResult<bool> {
        lock_user_row(&self.db, id).await
    }

    async fn create_user(&self, user_data: NewUser) -> RepoResult<User> {
        let user = user::ActiveModel {
            id: Set(UserId::new().into()),
            username: Set(user_data.username.to_owned()),
            password_hash: Set(user_data.password),
            email: Set(user_data.email),
            display_name: Set(user_data.display_name),
            principal_id: Set(user_data
                .key_identity
                .as_ref()
                .map(|k| k.principal_id.clone())),
            identity_key: Set(user_data.key_identity.map(|k| k.identity_key)),
            ..Default::default()
        };
        let res = user::Entity::insert(user)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?
            .into();
        Ok(res)
    }

    async fn find_by_username(&self, username: &str) -> RepoResult<Option<User>> {
        // Matches the idx_user_username_lower_unique index.
        let model = user::Entity::find()
            .filter(Expr::cust_with_values(
                "LOWER(username) = $1",
                [sea_orm::Value::from(username.to_lowercase())],
            ))
            .filter(user::Column::DeletedAt.is_null())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn find_by_home_handle(&self, username: &str, domain: &str) -> RepoResult<Option<User>> {
        let model = user::Entity::find()
            .filter(user::Column::Kind.eq("remote"))
            .filter(user::Column::HomeHandleDomain.eq(domain.to_ascii_lowercase()))
            .filter(user::Column::HomeUsername.eq(username.to_ascii_lowercase()))
            .filter(user::Column::DeletedAt.is_null())
            .order_by_desc(user::Column::UpdatedAt)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn update_profile(&self, id: UserId, patch: UpdateProfilePatch) -> RepoResult<User> {
        let model = user::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or(RepoError::new(RepoErrorKind::NotFound))?;

        let mut active = model.into_active_model();

        if let Some(val) = patch.display_name {
            active.display_name = Set(val);
        }
        if let Some(val) = patch.avatar_url {
            active.avatar_url = Set(val);
        }
        if let Some(val) = patch.status_text {
            active.status_text = Set(val);
        }
        if let Some(val) = patch.bio {
            active.bio = Set(val);
        }

        active.updated_at = Set(chrono::Utc::now().into());

        let updated = user::Entity::update(active)
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(updated.into())
    }

    /// Find a user by email address using a case-insensitive comparison.
    ///
    /// Applies `LOWER(email) = LOWER($1)` so that input casing is irrelevant.
    ///
    /// # Errors
    /// Propagates any underlying database error as [`rim_core::repository::error::RepoError`].
    async fn find_by_email(&self, email: &str) -> RepoResult<Option<User>> {
        let lower_email = email.to_lowercase();
        // Use a custom SQL expression: LOWER(email) = $1
        // This mirrors the idx_user_email_lower_unique index on the schema layer.
        let model = user::Entity::find()
            .filter(Expr::cust_with_values(
                "LOWER(email) = $1",
                [sea_orm::Value::from(lower_email)],
            ))
            .filter(user::Column::DeletedAt.is_null())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn update_password(&self, id: UserId, password_hash: String) -> RepoResult<()> {
        let model = user::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or(RepoError::new(RepoErrorKind::NotFound))?;

        let mut active = model.into_active_model();
        active.password_hash = Set(Some(password_hash));
        active.updated_at = Set(chrono::Utc::now().into());
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn record_terms_acceptance(
        &self,
        id: UserId,
        url: String,
        accepted_at: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<()> {
        let result = user::Entity::update_many()
            .col_expr(user::Column::TermsAcceptedAt, Expr::value(accepted_at))
            .col_expr(user::Column::TermsAcceptedUrl, Expr::value(url))
            .filter(user::Column::Id.eq(sea_orm::prelude::Uuid::from(id)))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        if result.rows_affected == 0 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }
        Ok(())
    }

    async fn record_invite_chain(
        &self,
        id: UserId,
        invite_id: Uuid,
        invited_by: Option<UserId>,
    ) -> RepoResult<()> {
        let result = user::Entity::update_many()
            .col_expr(user::Column::CreatedByInviteId, Expr::value(invite_id))
            .col_expr(
                user::Column::InvitedByUserId,
                Expr::value(invited_by.map(Uuid::from)),
            )
            .filter(user::Column::Id.eq(Uuid::from(id)))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        if result.rows_affected == 0 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }
        Ok(())
    }

    async fn invite_chain(&self, id: UserId) -> RepoResult<Option<InviteChain>> {
        let model = user::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.and_then(|m| {
            m.created_by_invite_id.map(|invite_id| InviteChain {
                invite_id,
                invited_by_user_id: m.invited_by_user_id.map(Into::into),
            })
        }))
    }

    async fn invited_users(&self, id: UserId) -> RepoResult<Vec<UserId>> {
        Ok(IdRow::find_by_statement(statement(
            r#"SELECT id FROM "user" WHERE invited_by_user_id = $1 ORDER BY created_at, id"#,
            vec![Uuid::from(id).into()],
        ))
        .all(&self.db)
        .await
        .map_err(map_db_err)?
        .into_iter()
        .map(|r| r.id.into())
        .collect())
    }

    async fn invites_created_by(&self, id: UserId) -> RepoResult<Vec<CreatedInvite>> {
        Ok(CreatedInviteRow::find_by_statement(statement(
            "SELECT id, 'instance' AS kind, created_at, expires_at, max_uses, use_count, \
             revoked_at FROM instance_invite WHERE created_by_user_id = $1 \
             UNION ALL SELECT id, 'workspace' AS kind, created_at, expires_at, max_uses, \
             use_count, revoked_at FROM invite WHERE created_by_user_id = $1 \
             ORDER BY created_at DESC, id",
            vec![Uuid::from(id).into()],
        ))
        .all(&self.db)
        .await
        .map_err(map_db_err)?
        .into_iter()
        .map(|r| CreatedInvite {
            id: r.id,
            kind: if r.kind == "instance" {
                InviteKind::Instance
            } else {
                InviteKind::Workspace
            },
            created_at: r.created_at.into(),
            expires_at: r.expires_at.map(Into::into),
            max_uses: r.max_uses,
            use_count: r.use_count,
            revoked_at: r.revoked_at.map(Into::into),
        })
        .collect())
    }

    async fn revoke_invites_created_by(&self, id: UserId) -> RepoResult<u64> {
        let mut revoked = 0;
        for table in ["instance_invite", "invite"] {
            revoked += self
                .db
                .execute_raw(statement(
                    &format!(
                        "UPDATE {table} SET revoked_at = now() WHERE created_by_user_id = $1 \
                         AND revoked_at IS NULL AND use_count < max_uses"
                    ),
                    vec![Uuid::from(id).into()],
                ))
                .await
                .map_err(map_db_err)?
                .rows_affected();
        }
        Ok(revoked)
    }

    async fn block_email(&self, email: &str, reason: &str) -> RepoResult<bool> {
        let row = FlagRow::find_by_statement(statement(
            "INSERT INTO blocked_email (email, reason) VALUES (lower($1), $2) \
             ON CONFLICT (email) DO UPDATE SET reason = EXCLUDED.reason \
             RETURNING (xmax = 0) AS flag",
            vec![email.trim().into(), reason.into()],
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(row.is_some_and(|r| r.flag))
    }

    async fn unblock_email(&self, email: &str) -> RepoResult<bool> {
        Ok(self
            .db
            .execute_raw(statement(
                "DELETE FROM blocked_email WHERE email = lower($1)",
                vec![email.trim().into()],
            ))
            .await
            .map_err(map_db_err)?
            .rows_affected()
            == 1)
    }

    async fn is_email_blocked(&self, email: &str) -> RepoResult<bool> {
        let row = FlagRow::find_by_statement(statement(
            "SELECT EXISTS (SELECT 1 FROM blocked_email WHERE email = lower($1)) AS flag",
            vec![email.trim().into()],
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(row.is_some_and(|r| r.flag))
    }

    async fn is_instance_operator(&self, id: UserId) -> RepoResult<bool> {
        let row = FlagRow::find_by_statement(statement(
            r#"SELECT EXISTS (SELECT 1 FROM "user" WHERE id = $1 AND instance_operator
               AND deleted_at IS NULL) AS flag"#,
            vec![Uuid::from(id).into()],
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(row.is_some_and(|r| r.flag))
    }

    async fn set_instance_operator(&self, id: UserId, operator: bool) -> RepoResult<bool> {
        Ok(self
            .db
            .execute_raw(statement(
                r#"UPDATE "user" SET instance_operator = $2, updated_at = now()
                   WHERE id = $1 AND instance_operator <> $2"#,
                vec![Uuid::from(id).into(), operator.into()],
            ))
            .await
            .map_err(map_db_err)?
            .rows_affected()
            == 1)
    }

    async fn instance_operators(&self) -> RepoResult<Vec<User>> {
        let models = user::Entity::find()
            .filter(user::Column::InstanceOperator.eq(true))
            .filter(user::Column::DeletedAt.is_null())
            .order_by_asc(user::Column::Username)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }
}
