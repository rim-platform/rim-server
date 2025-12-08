use async_trait::async_trait;
use sea_orm::{ColumnTrait, DbConn, EntityTrait, JoinType, QueryFilter, QueryOrder, QuerySelect, RelationTrait, Set};
use sea_orm::prelude::Uuid;
use entity::{server, server_member};
use entity::server::Model;
use rim_core::repository::repository::Repository;
use rim_core::server::model::{Server, ServerId};
use rim_core::server::repository::ServerRepository;
use rim_core::user::model::UserId;
use crate::impl_repository_for_entity;

pub struct SeaOrmServerRepository {
    db: DbConn,
}

impl SeaOrmServerRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

impl_repository_for_entity!(SeaOrmServerRepository, server::Entity, Server, ServerId);

#[async_trait]
impl ServerRepository for SeaOrmServerRepository {
    async fn create_server(&self, name: &str) -> anyhow::Result<Self::Model> {
        let new_server = server::ActiveModel {
            name: Set(name.to_owned()),
            ..Default::default()
        };

        let new_server = server::Entity::insert(new_server)
            .exec_with_returning(&self.db)
            .await?;

        Ok(new_server.into())
    }

    async fn find_by_name(&self, name: &str) -> anyhow::Result<Option<Self::Model>> {

        let res = server::Entity::find()
            .filter(server::Column::Name.eq(name.to_owned()))
            .one(&self.db)
            .await?;

        Ok(res.map(Into::into))
    }

    async fn get_servers_for_user(&self, user_id: UserId) -> anyhow::Result<Vec<Self::Model>> {
        let servers = server::Entity::find()
            .join(
                JoinType::InnerJoin,
                server_member::Relation::Server.def(),
            )
            .filter(server_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .all(&self.db)
            .await?;

        Ok(servers.into_iter().map(Into::into).collect())
    }
}
