use async_trait::async_trait;
use entity::{server, server_member};
use rim_core::repository::error::RepoResult;
use rim_core::server::model::{Server, ServerId};
use rim_core::server::repository::ServerRepository;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ColumnTrait, DbConn, EntityTrait, JoinType, QueryFilter, QuerySelect, RelationTrait, Set,
};

use crate::error::map_db_err;

pub struct SeaOrmServerRepository {
    db: DbConn,
}

impl SeaOrmServerRepository {
    pub fn new(db: DbConn) -> Self {
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
        let new_server = server::ActiveModel {
            name: Set(name.to_owned()),
            ..Default::default()
        };

        let new_server = server::Entity::insert(new_server)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(new_server.into())
    }

    async fn find_by_name(&self, name: &str) -> RepoResult<Option<Server>> {
        let res = server::Entity::find()
            .filter(server::Column::Name.eq(name.to_owned()))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(res.map(Into::into))
    }

    async fn get_servers_for_user(&self, user_id: UserId) -> RepoResult<Vec<Server>> {
        let servers = server::Entity::find()
            .join(JoinType::InnerJoin, server_member::Relation::Server.def())
            .filter(server_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(servers.into_iter().map(Into::into).collect())
    }
}
