use async_trait::async_trait;
use entity::{server, server_member};
use rim_core::repository::error::RepoResult;
use rim_core::server::model::{DeletedWorkspaces, Server, ServerId};
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, FromQueryResult, JoinType,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, RelationTrait, Set, Statement,
};

use crate::DbHandle;
use crate::error::map_db_err;

/// `server.kind` of a workspace; the relay's direct space is never listed or found by name.
const WORKSPACE_KIND: &str = "workspace";

pub struct SeaOrmServerRepository {
    db: DbHandle,
}

impl SeaOrmServerRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ServerRepository for SeaOrmServerRepository {
    async fn get_by_id(&self, id: ServerId) -> RepoResult<Option<Server>> {
        let model = server::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn create_server(&self, name: &str) -> RepoResult<Server> {
        // Only the columns a workspace sets; `kind` keeps its default and is not read back.
        let id = ServerId::new();
        let now = chrono::Utc::now();
        let new_server = server::ActiveModel {
            id: Set(id.into()),
            name: Set(name.to_owned()),
            created_at: Set(now.into()),
            updated_at: Set(now.into()),
            ..Default::default()
        };
        server::Entity::insert(new_server)
            .exec_without_returning(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(Server {
            id,
            name: name.to_owned(),
            created_at: now,
            updated_at: now,
        })
    }

    async fn get_servers_for_user(&self, user_id: UserId) -> RepoResult<Vec<Server>> {
        let servers = server::Entity::find()
            .join(JoinType::InnerJoin, server::Relation::ServerMember.def())
            .filter(server_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(server::Column::Kind.eq(WORKSPACE_KIND))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(servers.into_iter().map(Into::into).collect())
    }

    async fn find_owned_by_name(&self, owner: UserId, name: &str) -> RepoResult<Option<Server>> {
        let model = server::Entity::find()
            .join(JoinType::InnerJoin, server::Relation::ServerMember.def())
            .filter(server_member::Column::UserId.eq::<Uuid>(owner.into()))
            .filter(server_member::Column::MemberRole.eq(
                entity::sea_orm_active_enums::ServerMemberRole::from(ServerMemberRole::Owner),
            ))
            .filter(server::Column::Kind.eq(WORKSPACE_KIND))
            .filter(Expr::cust_with_values(
                r#"lower(trim("server"."name")) = lower(trim($1))"#,
                [sea_orm::Value::from(name.to_owned())],
            ))
            .order_by_asc(server::Column::CreatedAt)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn count_owned(&self, owner: UserId) -> RepoResult<u64> {
        server::Entity::find()
            .join(JoinType::InnerJoin, server::Relation::ServerMember.def())
            .filter(server_member::Column::UserId.eq::<Uuid>(owner.into()))
            .filter(server_member::Column::MemberRole.eq(
                entity::sea_orm_active_enums::ServerMemberRole::from(ServerMemberRole::Owner),
            ))
            .filter(server::Column::Kind.eq(WORKSPACE_KIND))
            .count(&self.db)
            .await
            .map_err(map_db_err)
    }

    async fn delete_workspaces(&self, ids: &[ServerId]) -> RepoResult<DeletedWorkspaces> {
        delete_workspaces(&self.db, ids).await
    }
}

/// The stored files of the workspaces in `$1`: attachments past their upload, custom emoji and
/// plugin bundles.
const WORKSPACE_FILES: &[&str] = &[
    "SELECT a.storage_url AS value FROM message_attachment a \
     JOIN message m ON m.id = a.message_id \
     JOIN conversation c ON c.id = m.conversation_id \
     WHERE c.server_id = ANY($1) AND a.storage_url NOT LIKE 'pending://%'",
    "SELECT image_url AS value FROM custom_emoji WHERE server_id = ANY($1)",
    "SELECT storage_key AS value FROM plugin_bundle WHERE server_id = ANY($1)",
];

#[derive(FromQueryResult)]
struct FileRow {
    value: String,
}

/// Delete the workspaces in `ids` and everything in them; the rows that reference a workspace
/// go by cascade, its keybinds (which name it without a foreign key) explicitly. The direct
/// space is left alone. Shared by the workspace deletion and the account purge.
pub(crate) async fn delete_workspaces(
    db: &DbHandle,
    ids: &[ServerId],
) -> RepoResult<DeletedWorkspaces> {
    let statement = |sql: &str, values: Vec<sea_orm::Value>| {
        Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
    };
    let ids: Vec<Uuid> = ids.iter().copied().map(Into::into).collect();
    // Only workspaces: the direct space is filtered out before anything is read or deleted.
    let ids: Vec<Uuid> = IdRow::find_by_statement(statement(
        "SELECT id FROM server WHERE id = ANY($1) AND kind = 'workspace'",
        vec![ids.into()],
    ))
    .all(db)
    .await
    .map_err(map_db_err)?
    .into_iter()
    .map(|r| r.id)
    .collect();
    if ids.is_empty() {
        return Ok(DeletedWorkspaces::default());
    }
    let mut storage_keys = Vec::new();
    for sql in WORKSPACE_FILES {
        storage_keys.extend(
            FileRow::find_by_statement(statement(sql, vec![ids.clone().into()]))
                .all(db)
                .await
                .map_err(map_db_err)?
                .into_iter()
                .map(|r| r.value),
        );
    }
    db.execute_raw(statement(
        "DELETE FROM keybind WHERE scope = 'workspace' AND subject_id = ANY($1)",
        vec![ids.clone().into()],
    ))
    .await
    .map_err(map_db_err)?;
    let deleted = db
        .execute_raw(statement(
            "DELETE FROM server WHERE id = ANY($1)",
            vec![ids.into()],
        ))
        .await
        .map_err(map_db_err)?
        .rows_affected();
    Ok(DeletedWorkspaces {
        deleted,
        storage_keys,
    })
}

#[derive(FromQueryResult)]
struct IdRow {
    id: Uuid,
}
