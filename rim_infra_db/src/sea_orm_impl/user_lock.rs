use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{ConnectionTrait, DbBackend, Statement};

use crate::DbHandle;
use crate::error::map_db_err;

/// Lock the user's row until the surrounding transaction ends (`SELECT ... FOR UPDATE`).
/// Returns `false` when the user does not exist.
pub(crate) async fn lock_user_row(db: &DbHandle, user_id: UserId) -> RepoResult<bool> {
    let id: Uuid = user_id.into();
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"SELECT id FROM "user" WHERE id = $1 FOR UPDATE"#,
            [id.into()],
        ))
        .await
        .map_err(map_db_err)?;
    Ok(row.is_some())
}
