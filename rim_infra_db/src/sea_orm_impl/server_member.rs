use async_trait::async_trait;
use entity::server_member;
use rim_core::repository::error::RepoResult;
use rim_core::server::model::ServerId;
use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, DbConn, EntityTrait, QueryFilter, Set};

use crate::error::map_db_err;
use rim_core::repository::error::{RepoError, RepoErrorKind};

pub struct SeaOrmServerMemberRepository {
    pub db: DbConn,
}

impl SeaOrmServerMemberRepository {
    pub fn new(db: DbConn) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ServerMemberRepository for SeaOrmServerMemberRepository {
    async fn get_by_id(&self, id: ServerMemberId) -> RepoResult<Option<ServerMember>> {
        let model = server_member::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn get_member(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> RepoResult<Option<ServerMember>> {
        let model = server_member::Entity::find()
            .filter(server_member::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(server_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn add_member(
        &self,
        server_id: ServerId,
        user_id: UserId,
        role: ServerMemberRole,
    ) -> RepoResult<ServerMember> {
        let active = server_member::ActiveModel {
            id: Set(ServerMemberId::new().into()),
            server_id: Set(server_id.into()),
            user_id: Set(user_id.into()),
            member_role: Set(role.into()),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn change_member_role(
        &self,
        server_id: ServerId,
        user_id: UserId,
        role: ServerMemberRole,
    ) -> RepoResult<()> {
        let member = server_member::Entity::find()
            .filter(server_member::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(server_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        let Some(member) = member else {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        };

        let mut active: server_member::ActiveModel = member.into();
        active.member_role = Set(role.into());
        active.update(&self.db).await.map_err(map_db_err)?;

        Ok(())
    }

    async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()> {
        let res = server_member::Entity::delete_many()
            .filter(server_member::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(server_member::Column::UserId.eq::<Uuid>(user_id.into()))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        if res.rows_affected == 0 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }

        Ok(())
    }

    async fn get_members_for_server(&self, server_id: ServerId) -> RepoResult<Vec<ServerMember>> {
        let members = server_member::Entity::find()
            .filter(server_member::Column::ServerId.eq::<Uuid>(server_id.into()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(members.into_iter().map(Into::into).collect())
    }

    async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool> {
        Ok(self.get_member(server_id, user_id).await?.is_some())
    }
}
