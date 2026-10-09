use async_trait::async_trait;
use chrono::Duration;
use entity::{plugin_bundle, workspace_plugin};
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use rim_core::workspace_plugin::model::{
    NewPluginBundle, PluginBundle, PluginRuntimeKind, PluginSource, StatementText, WorkspacePlugin,
    WorkspacePluginId, WorkspacePluginStatus, WorkspacePluginWrite,
};
use rim_core::workspace_plugin::repository::WorkspacePluginRepository;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, FromQueryResult, QueryFilter, QueryOrder,
    QueryResult, Statement, Value,
};
use serde_json::json;

use crate::DbHandle;
use crate::error::map_db_err;

/// The columns an add or update writes, in the order of [`write_values`], starting at `$2`.
const WRITE_COLUMNS: &str = "server_id, plugin_id, version, display_name, runtime_kind, \
     status, source_kind, git_url, git_commit, git_tree_sha256, bundle_url, bundle_sha256, \
     bundle_size, statement_payload, statement_signature, publisher_principal_id, \
     publisher_key_seq, publisher_key_log, approved_by";

const WRITE_COLUMN_COUNT: usize = 19;

fn invalid_data() -> RepoError {
    RepoError::new(RepoErrorKind::InvalidData)
}

fn statements_to_json(log: &[StatementText]) -> serde_json::Value {
    log.iter()
        .map(|s| json!({ "payload": s.payload, "signature": s.signature }))
        .collect()
}

fn statements_from_json(value: serde_json::Value) -> RepoResult<Vec<StatementText>> {
    let serde_json::Value::Array(items) = value else {
        return Err(invalid_data());
    };
    items
        .iter()
        .map(|item| {
            let text = |field: &str| {
                item.get(field)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(invalid_data)
            };
            Ok(StatementText {
                payload: text("payload")?,
                signature: text("signature")?,
            })
        })
        .collect()
}

/// The values of [`WRITE_COLUMNS`].
fn write_values(entry: WorkspacePluginWrite) -> RepoResult<Vec<Value>> {
    let (source_kind, git_url, git_commit, git_tree, bundle_url, bundle_sha256, bundle_size) =
        match entry.source {
            PluginSource::Bundle { url, sha256, size } => (
                "bundle",
                None,
                None,
                None,
                Some(url),
                Some(sha256),
                Some(i64::try_from(size).map_err(|_| invalid_data())?),
            ),
            PluginSource::Git {
                url,
                commit,
                tree_sha256,
            } => (
                "git",
                Some(url),
                Some(commit),
                Some(tree_sha256),
                None,
                None,
                None,
            ),
        };
    let key_seq = i32::try_from(entry.publisher_key_seq).map_err(|_| invalid_data())?;
    let key_log = entry.publisher_key_log.as_deref().map(statements_to_json);
    Ok(vec![
        Uuid::from(entry.server_id).into(),
        entry.plugin_id.into(),
        entry.version.into(),
        entry.display_name.into(),
        entry.runtime_kind.as_str().into(),
        entry.status.as_str().into(),
        source_kind.into(),
        git_url.into(),
        git_commit.into(),
        git_tree.into(),
        bundle_url.into(),
        bundle_sha256.into(),
        bundle_size.into(),
        entry.statement.payload.into(),
        entry.statement.signature.into(),
        entry.publisher_principal_id.into(),
        key_seq.into(),
        key_log.into(),
        Uuid::from(entry.approved_by).into(),
    ])
}

fn to_source(m: &workspace_plugin::Model) -> RepoResult<PluginSource> {
    match m.source_kind.as_str() {
        "bundle" => Ok(PluginSource::Bundle {
            url: m.bundle_url.clone().ok_or_else(invalid_data)?,
            sha256: m.bundle_sha256.clone().ok_or_else(invalid_data)?,
            size: m
                .bundle_size
                .and_then(|s| u64::try_from(s).ok())
                .ok_or_else(invalid_data)?,
        }),
        "git" => Ok(PluginSource::Git {
            url: m.git_url.clone().ok_or_else(invalid_data)?,
            commit: m.git_commit.clone().ok_or_else(invalid_data)?,
            tree_sha256: m.git_tree_sha256.clone().ok_or_else(invalid_data)?,
        }),
        _ => Err(invalid_data()),
    }
}

fn to_entry(m: workspace_plugin::Model) -> RepoResult<WorkspacePlugin> {
    let source = to_source(&m)?;
    Ok(WorkspacePlugin {
        id: WorkspacePluginId::from(m.id),
        server_id: m.server_id.into(),
        plugin_id: m.plugin_id,
        version: m.version,
        display_name: m.display_name,
        runtime_kind: PluginRuntimeKind::parse(&m.runtime_kind).ok_or_else(invalid_data)?,
        status: WorkspacePluginStatus::parse(&m.status).ok_or_else(invalid_data)?,
        source,
        statement: StatementText {
            payload: m.statement_payload,
            signature: m.statement_signature,
        },
        publisher_principal_id: m.publisher_principal_id,
        publisher_key_seq: u32::try_from(m.publisher_key_seq).map_err(|_| invalid_data())?,
        publisher_key_log: m.publisher_key_log.map(statements_from_json).transpose()?,
        approved_by: m.approved_by.map(Into::into),
        approved_at: m.approved_at.into(),
        created_at: m.created_at.into(),
        updated_at: m.updated_at.into(),
    })
}

fn row_to_entry(row: &QueryResult) -> RepoResult<WorkspacePlugin> {
    to_entry(workspace_plugin::Model::from_query_result(row, "").map_err(map_db_err)?)
}

fn to_bundle(m: plugin_bundle::Model) -> RepoResult<PluginBundle> {
    Ok(PluginBundle {
        server_id: m.server_id.into(),
        sha256: m.sha256,
        size_bytes: u64::try_from(m.size_bytes).map_err(|_| invalid_data())?,
        storage_key: m.storage_key,
        uploaded_by: m.uploaded_by.map(Into::into),
        created_at: m.created_at.into(),
    })
}

/// Seconds as `make_interval` takes them.
fn seconds(duration: Duration) -> Value {
    (duration.num_milliseconds() as f64 / 1000.0).into()
}

pub struct SeaOrmWorkspacePluginRepository {
    db: DbHandle,
}

impl SeaOrmWorkspacePluginRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    async fn query_one(&self, sql: &str, values: Vec<Value>) -> RepoResult<Option<QueryResult>> {
        self.db
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                values,
            ))
            .await
            .map_err(map_db_err)
    }
}

#[async_trait]
impl WorkspacePluginRepository for SeaOrmWorkspacePluginRepository {
    async fn lock_server(&self, server_id: ServerId) -> RepoResult<bool> {
        let row = self
            .query_one(
                "SELECT id FROM server WHERE id = $1 FOR UPDATE",
                vec![Uuid::from(server_id).into()],
            )
            .await?;
        Ok(row.is_some())
    }

    async fn list(&self, server_id: ServerId) -> RepoResult<Vec<WorkspacePlugin>> {
        workspace_plugin::Entity::find()
            .filter(workspace_plugin::Column::ServerId.eq(Uuid::from(server_id)))
            .order_by_asc(workspace_plugin::Column::PluginId)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(to_entry)
            .collect()
    }

    async fn get(
        &self,
        server_id: ServerId,
        plugin_id: &str,
    ) -> RepoResult<Option<WorkspacePlugin>> {
        workspace_plugin::Entity::find()
            .filter(workspace_plugin::Column::ServerId.eq(Uuid::from(server_id)))
            .filter(workspace_plugin::Column::PluginId.eq(plugin_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .map(to_entry)
            .transpose()
    }

    async fn insert(
        &self,
        entry: WorkspacePluginWrite,
        max_entries: u64,
    ) -> RepoResult<Option<WorkspacePlugin>> {
        let mut values: Vec<Value> = vec![Uuid::from(WorkspacePluginId::new()).into()];
        values.extend(write_values(entry)?);
        values.push(i64::try_from(max_entries).unwrap_or(i64::MAX).into());
        let placeholders: Vec<String> = (2..=WRITE_COLUMN_COUNT + 1)
            .map(|i| format!("${i}"))
            .collect();
        let sql = format!(
            "INSERT INTO workspace_plugin (id, {WRITE_COLUMNS}, approved_at) \
             SELECT $1, {}, now() \
             WHERE (SELECT count(*) FROM workspace_plugin WHERE server_id = $2) < ${} \
             RETURNING *",
            placeholders.join(", "),
            WRITE_COLUMN_COUNT + 2,
        );
        self.query_one(&sql, values)
            .await?
            .as_ref()
            .map(row_to_entry)
            .transpose()
    }

    async fn update(
        &self,
        id: WorkspacePluginId,
        entry: WorkspacePluginWrite,
    ) -> RepoResult<Option<WorkspacePlugin>> {
        let mut values: Vec<Value> = vec![Uuid::from(id).into()];
        values.extend(write_values(entry)?);
        let assignments: Vec<String> = WRITE_COLUMNS
            .split(", ")
            .enumerate()
            .map(|(i, column)| format!("{column} = ${}", i + 2))
            .collect();
        let sql = format!(
            "UPDATE workspace_plugin SET {}, approved_at = now(), updated_at = now() \
             WHERE id = $1 RETURNING *",
            assignments.join(", "),
        );
        self.query_one(&sql, values)
            .await?
            .as_ref()
            .map(row_to_entry)
            .transpose()
    }

    async fn delete(&self, server_id: ServerId, plugin_id: &str) -> RepoResult<bool> {
        let res = workspace_plugin::Entity::delete_many()
            .filter(workspace_plugin::Column::ServerId.eq(Uuid::from(server_id)))
            .filter(workspace_plugin::Column::PluginId.eq(plugin_id))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected > 0)
    }

    async fn get_bundle(
        &self,
        server_id: ServerId,
        sha256: &str,
    ) -> RepoResult<Option<PluginBundle>> {
        plugin_bundle::Entity::find_by_id((Uuid::from(server_id), sha256.to_string()))
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .map(to_bundle)
            .transpose()
    }

    async fn insert_bundle(&self, bundle: NewPluginBundle) -> RepoResult<Option<PluginBundle>> {
        let size = i64::try_from(bundle.size_bytes).map_err(|_| invalid_data())?;
        let row = self
            .query_one(
                "INSERT INTO plugin_bundle (server_id, sha256, size_bytes, storage_key, uploaded_by) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (server_id, sha256) DO NOTHING \
                 RETURNING *",
                vec![
                    Uuid::from(bundle.server_id).into(),
                    bundle.sha256.into(),
                    size.into(),
                    bundle.storage_key.into(),
                    Uuid::from(bundle.uploaded_by).into(),
                ],
            )
            .await?;
        row.as_ref()
            .map(|r| {
                plugin_bundle::Model::from_query_result(r, "")
                    .map_err(map_db_err)
                    .and_then(to_bundle)
            })
            .transpose()
    }

    async fn bundle_bytes(&self, server_id: ServerId) -> RepoResult<u64> {
        let row = self
            .query_one(
                "SELECT COALESCE(SUM(size_bytes), 0)::bigint AS total \
                 FROM plugin_bundle WHERE server_id = $1",
                vec![Uuid::from(server_id).into()],
            )
            .await?;
        let total: i64 = match row {
            Some(r) => r.try_get("", "total").map_err(map_db_err)?,
            None => 0,
        };
        Ok(u64::try_from(total).unwrap_or(0))
    }

    async fn delete_unnamed_bundles(
        &self,
        server_id: ServerId,
        age: Duration,
    ) -> RepoResult<Vec<String>> {
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM plugin_bundle b \
                 WHERE b.server_id = $1 AND b.created_at < now() - make_interval(secs => $2) \
                     AND NOT EXISTS (SELECT 1 FROM workspace_plugin w \
                         WHERE w.server_id = b.server_id AND w.bundle_sha256 = b.sha256) \
                 RETURNING b.storage_key",
                [Uuid::from(server_id).into(), seconds(age)],
            ))
            .await
            .map_err(map_db_err)?;
        rows.iter()
            .map(|r| r.try_get::<String>("", "storage_key").map_err(map_db_err))
            .collect()
    }
}
