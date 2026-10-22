use async_trait::async_trait;
use entity::{server_member, server_member_timeout};
use rim_core::repository::error::RepoResult;
use rim_core::server::model::ServerId;
use rim_core::server_member::model::{
    MemberStanding, ServerMember, ServerMemberId, ServerMemberRole,
};
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, Statement,
};

use crate::DbHandle;
use crate::error::map_db_err;
use rim_core::repository::error::{RepoError, RepoErrorKind};

pub struct SeaOrmServerMemberRepository {
    pub db: DbHandle,
}

impl SeaOrmServerMemberRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ServerMemberRepository for SeaOrmServerMemberRepository {
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

    async fn get_members(
        &self,
        server_id: ServerId,
        user_ids: &[UserId],
    ) -> RepoResult<Vec<ServerMember>> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        let models = server_member::Entity::find()
            .filter(server_member::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(server_member::Column::UserId.is_in(user_ids.iter().map(|id| Uuid::from(*id))))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(Into::into).collect())
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

    async fn join_direct_space(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool> {
        let inserted = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO server_member (id, server_id, user_id, member_role, joined_at) \
                 SELECT $1, s.id, u.id, 'guest'::server_member_role, now() \
                 FROM server s, \"user\" u \
                 WHERE s.id = $2 AND s.kind = 'direct' AND u.id = $3 AND u.kind = 'local' \
                 ON CONFLICT DO NOTHING",
                [
                    Uuid::from(ServerMemberId::new()).into(),
                    Uuid::from(server_id).into(),
                    Uuid::from(user_id).into(),
                ],
            ))
            .await
            .map_err(map_db_err)?
            .rows_affected();
        // A concurrent first use may have inserted the row this one skipped.
        Ok(inserted == 1 || self.is_member(server_id, user_id).await?)
    }

    async fn get_owner_id(&self, server_id: ServerId) -> RepoResult<Option<UserId>> {
        let owner: Option<Uuid> = server_member::Entity::find()
            .select_only()
            .column(server_member::Column::UserId)
            .filter(server_member::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(server_member::Column::MemberRole.eq(
                entity::sea_orm_active_enums::ServerMemberRole::from(ServerMemberRole::Owner),
            ))
            .order_by_asc(server_member::Column::JoinedAt)
            .into_tuple()
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(owner.map(Into::into))
    }

    async fn get_member_standing(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> RepoResult<Option<MemberStanding>> {
        let Some(member) = self.get_member(server_id, user_id).await? else {
            return Ok(None);
        };
        let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
        let timeout = server_member_timeout::Entity::find()
            .filter(server_member_timeout::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(server_member_timeout::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(server_member_timeout::Column::Until.gt(now))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(Some(MemberStanding {
            role: member.member_role,
            timed_out_until: timeout.map(|t| t.until.into()),
        }))
    }

    async fn count_members(&self, server_id: ServerId) -> RepoResult<u64> {
        server_member::Entity::find()
            .filter(server_member::Column::ServerId.eq::<Uuid>(server_id.into()))
            .count(&self.db)
            .await
            .map_err(map_db_err)
    }
}
