use async_trait::async_trait;
use sea_orm::{ColumnTrait, DbConn, EntityTrait, JoinType, QueryFilter, QueryOrder, QuerySelect, RelationTrait, Set};
use entity::{server, server_member};
use entity::server::Model;
use rim_core::repositories::server_repository::ServerRepository;
use rim_core::repositories::repository::Repository;
use crate::impl_repository_for_entity;

pub struct SeaOrmServerRepository {
    db: DbConn,
}

impl SeaOrmServerRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

type Id = i32;

impl_repository_for_entity!(SeaOrmServerRepository, server::Model, server::Column, Id, server::Entity);

#[async_trait]
impl ServerRepository<server::Model, Id> for SeaOrmServerRepository {
    async fn create_server(&self, name: &str) -> anyhow::Result<Model> {
        let new_server = server::ActiveModel {
            name: Set(name.to_owned()),
            ..Default::default()
        };

        let new_server = server::Entity::insert(new_server)
            .exec_with_returning(&self.db)
            .await?;

        Ok(new_server)
    }

    async fn find_by_name(&self, name: &str) -> anyhow::Result<Option<Model>> {

        let res = server::Entity::find()
            .filter(server::Column::Name.eq(name.to_owned()))
            .one(&self.db)
            .await?;

        Ok(res)
    }

    async fn get_servers_for_user(&self, user_id: Id) -> anyhow::Result<Vec<Model>> {
        let servers = server::Entity::find()
            .join(
                JoinType::InnerJoin,
                server_member::Relation::Server.def(),
            )
            .filter(server_member::Column::UserId.eq(user_id))
            .all(&self.db)
            .await?;

        Ok(servers)
    }
}
