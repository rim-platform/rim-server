use async_trait::async_trait;
use sea_orm::{DbConn, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set, ActiveModelTrait};
use entity::sea_orm_active_enums::ServerMemberRole;
use entity::server_member;
use entity::server_member::Model;
use rim_core::repositories::server_member_repository::ServerMemberRepository;
use rim_core::repositories::repository::Repository;
use crate::impl_repository_for_entity;


pub struct SeaOrmServerMemberRepository {
    pub db: DbConn,
}

impl SeaOrmServerMemberRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

type Id = i32;

impl_repository_for_entity!(
    SeaOrmServerMemberRepository,
    server_member::Model,
    server_member::Column,
    Id,
    server_member::Entity
);

#[async_trait]
impl ServerMemberRepository<server_member::Model, Id, entity::sea_orm_active_enums::ServerMemberRole>
for SeaOrmServerMemberRepository {
    async fn add_member(&self, server_id: i32, user_id: i32, role: ServerMemberRole) -> anyhow::Result<Model> {
        let active = server_member::ActiveModel {
            server_id: Set(server_id),
            user_id: Set(user_id),
            member_role: Set(role),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await?;
        Ok(inserted)
    }

    async fn change_member_role(&self, server_id: i32, user_id: i32, role: ServerMemberRole) -> anyhow::Result<()> {
        if let Some(member) = server_member::Entity::find()
            .filter(server_member::Column::ServerId.eq(server_id))
            .filter(server_member::Column::UserId.eq(user_id))
            .one(&self.db)
            .await?
        {
            let mut active: server_member::ActiveModel = member.into();
            active.member_role = Set(role);
            active.update(&self.db).await?;
        }

        Ok(())
    }

    async fn remove_member(&self, server_id: i32, user_id: i32) -> anyhow::Result<()> {
        server_member::Entity::delete_many()
            .filter(server_member::Column::ServerId.eq(server_id))
            .filter(server_member::Column::UserId.eq(user_id))
            .exec(&self.db)
            .await?;
        Ok(())
    }

    async fn get_members_for_server(&self, server_id: i32) -> anyhow::Result<Vec<Model>> {
        let members = server_member::Entity::find()
            .filter(server_member::Column::ServerId.eq(server_id))
            .all(&self.db)
            .await?;

        Ok(members)
    }
}