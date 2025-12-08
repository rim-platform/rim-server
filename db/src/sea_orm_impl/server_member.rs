use async_trait::async_trait;
use sea_orm::{DbConn, ColumnTrait, EntityTrait, QueryFilter, Set, ActiveModelTrait};
use sea_orm::prelude::Uuid;
use entity::server_member;
use rim_core::repository::repository::Repository;
use rim_core::server::model::ServerId;
use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use crate::impl_repository_for_entity;


pub struct SeaOrmServerMemberRepository {
    pub db: DbConn,
}

impl SeaOrmServerMemberRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

impl_repository_for_entity!(
    SeaOrmServerMemberRepository,
    server_member::Entity,
    ServerMember,
    ServerMemberId
);

#[async_trait]
impl ServerMemberRepository for SeaOrmServerMemberRepository {
    async fn add_member(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> anyhow::Result<Self::Model> {
        let active = server_member::ActiveModel {
            server_id: Set(server_id.into()),
            user_id: Set(user_id.into()),
            member_role: Set(role.into()),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await?;
        Ok(inserted.into())
    }

    async fn change_member_role(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> anyhow::Result<()> {
        if let Some(member) = server_member::Entity::find()
            .filter(server_member::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(server_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .one(&self.db)
            .await?
        {
            let mut active: server_member::ActiveModel = member.into();
            active.member_role = Set(role.into());
            active.update(&self.db).await?;
        }

        Ok(())
    }

    async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> anyhow::Result<()> {
        server_member::Entity::delete_many()
            .filter(server_member::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(server_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .exec(&self.db)
            .await?;
        Ok(())
    }

    async fn get_members_for_server(&self, server_id: ServerId) -> anyhow::Result<Vec<Self::Model>> {
        let members = server_member::Entity::find()
            .filter(server_member::Column::ServerId.eq::<Uuid>(server_id.into()))
            .all(&self.db)
            .await?;

        Ok(members.into_iter().map(Into::into).collect())
    }
}