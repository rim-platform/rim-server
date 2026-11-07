use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::{home_layout, user};
use rim_core::home_layout::model::HomeLayout;
use rim_core::home_layout::repository::HomeLayoutRepository;
use rim_core::host::model::UserKind;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{DbBackend, EntityTrait, QuerySelect, Statement};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmHomeLayoutRepository {
    db: DbHandle,
}

impl SeaOrmHomeLayoutRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

fn to_domain(model: home_layout::Model) -> HomeLayout {
    HomeLayout {
        user_id: model.user_id.into(),
        layout: model.layout,
        edited_at: model.edited_at.into(),
        updated_at: model.updated_at.into(),
    }
}

#[async_trait]
impl HomeLayoutRepository for SeaOrmHomeLayoutRepository {
    async fn account_kind(&self, user_id: UserId) -> RepoResult<Option<UserKind>> {
        let kind: Option<String> = user::Entity::find_by_id(Uuid::from(user_id))
            .select_only()
            .column(user::Column::Kind)
            .into_tuple()
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(kind.as_deref().and_then(UserKind::parse))
    }

    async fn get(&self, user_id: UserId) -> RepoResult<Option<HomeLayout>> {
        let model = home_layout::Entity::find_by_id(Uuid::from(user_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(to_domain))
    }

    async fn save_unless_newer(
        &self,
        user_id: UserId,
        layout: serde_json::Value,
        edited_at: DateTime<Utc>,
    ) -> RepoResult<Option<HomeLayout>> {
        let edited_at: DateTimeWithTimeZone = edited_at.into();
        let model = home_layout::Entity::find()
            .from_raw_sql(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"INSERT INTO home_layout (user_id, layout, edited_at, updated_at)
                   VALUES ($1, $2, $3, now())
                   ON CONFLICT (user_id) DO UPDATE
                   SET layout = EXCLUDED.layout,
                       edited_at = EXCLUDED.edited_at,
                       updated_at = EXCLUDED.updated_at
                   WHERE home_layout.edited_at <= EXCLUDED.edited_at
                   RETURNING user_id, layout, edited_at, updated_at"#,
                [Uuid::from(user_id).into(), layout.into(), edited_at.into()],
            ))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(to_domain))
    }

    async fn delete(&self, user_id: UserId) -> RepoResult<bool> {
        let result = home_layout::Entity::delete_by_id(Uuid::from(user_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(result.rows_affected > 0)
    }
}
