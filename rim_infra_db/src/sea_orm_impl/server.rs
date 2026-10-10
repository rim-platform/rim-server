use async_trait::async_trait;
use entity::{server, server_member};
use rim_core::repository::error::RepoResult;
use rim_core::server::model::{Server, ServerId};
use rim_core::server::repository::ServerRepository;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{ColumnTrait, EntityTrait, JoinType, QueryFilter, QuerySelect, RelationTrait, Set};

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
}
