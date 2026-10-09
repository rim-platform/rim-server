use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::{device_key, key_challenge, key_log_entry, profile_card, user};
use rim_core::key_identity::model::{
    DeviceKey, DeviceKeyId, KeyChallenge, KeyChallengeId, KeyLogEntry, NewDeviceKey,
    NewKeyLogEntry, StoredProfileCard,
};
use rim_core::key_identity::repository::{KeyChallengeRepository, KeyIdentityRepository};
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, QueryOrder, Set, Statement,
};

use crate::DbHandle;
use crate::error::map_db_err;
use crate::sea_orm_impl::user_lock::lock_user_row;

fn to_i32(value: u32) -> RepoResult<i32> {
    i32::try_from(value).map_err(|_| RepoError::new(RepoErrorKind::InvalidData))
}

fn new_log_entry_model(
    user_id: UserId,
    entry: NewKeyLogEntry,
) -> RepoResult<key_log_entry::ActiveModel> {
    Ok(key_log_entry::ActiveModel {
        user_id: Set(user_id.into()),
        seq: Set(to_i32(entry.seq)?),
        entry_type: Set(entry.entry_type.as_str().to_string()),
        payload: Set(entry.payload),
        signature: Set(entry.signature),
        ..Default::default()
    })
}

pub struct SeaOrmKeyIdentityRepository {
    db: DbHandle,
}

impl SeaOrmKeyIdentityRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl KeyIdentityRepository for SeaOrmKeyIdentityRepository {
    async fn lock_user(&self, user_id: UserId) -> RepoResult<bool> {
        lock_user_row(&self.db, user_id).await
    }

    async fn find_user_by_principal_id(&self, principal_id: &str) -> RepoResult<Option<UserId>> {
        let model = user::Entity::find()
            .filter(user::Column::PrincipalId.eq(principal_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(|m| m.id.into()))
    }

    async fn list_key_log(&self, user_id: UserId) -> RepoResult<Vec<KeyLogEntry>> {
        let models = key_log_entry::Entity::find()
            .filter(key_log_entry::Column::UserId.eq(Uuid::from(user_id)))
            .order_by_asc(key_log_entry::Column::Seq)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        models.into_iter().map(TryInto::try_into).collect()
    }

    async fn enroll(
        &self,
        user_id: UserId,
        principal_id: &str,
        identity_key: &str,
        entries: Vec<NewKeyLogEntry>,
    ) -> RepoResult<()> {
        let res = user::Entity::update_many()
            .col_expr(user::Column::PrincipalId, Expr::value(principal_id))
            .col_expr(user::Column::IdentityKey, Expr::value(identity_key))
            .col_expr(user::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(user::Column::Id.eq(Uuid::from(user_id)))
            .filter(user::Column::PrincipalId.is_null())
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        if res.rows_affected != 1 {
            return Err(RepoError::new(RepoErrorKind::Conflict));
        }
        let models = entries
            .into_iter()
            .map(|entry| new_log_entry_model(user_id, entry))
            .collect::<RepoResult<Vec<_>>>()?;
        key_log_entry::Entity::insert_many(models)
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn append_rotation(
        &self,
        user_id: UserId,
        entry: NewKeyLogEntry,
        identity_key: &str,
    ) -> RepoResult<()> {
        key_log_entry::Entity::insert(new_log_entry_model(user_id, entry)?)
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        user::Entity::update_many()
            .col_expr(user::Column::IdentityKey, Expr::value(identity_key))
            .col_expr(user::Column::UpdatedAt, Expr::value(Utc::now()))
            .filter(user::Column::Id.eq(Uuid::from(user_id)))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn add_device(&self, device: NewDeviceKey) -> RepoResult<DeviceKey> {
        let model = device_key::ActiveModel {
            id: Set(DeviceKeyId::new().into()),
            user_id: Set(device.user_id.into()),
            device_id: Set(device.device_id),
            public_key: Set(device.public_key),
            key_hash: Set(device.key_hash),
            name: Set(device.name),
            cert_payload: Set(device.cert_payload),
            cert_signature: Set(device.cert_signature),
            key_seq: Set(to_i32(device.key_seq)?),
            ..Default::default()
        };
        device_key::Entity::insert(model)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?
            .try_into()
    }

    async fn get_device(&self, user_id: UserId, device_id: Uuid) -> RepoResult<Option<DeviceKey>> {
        let model = device_key::Entity::find()
            .filter(device_key::Column::UserId.eq(Uuid::from(user_id)))
            .filter(device_key::Column::DeviceId.eq(device_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        model.map(TryInto::try_into).transpose()
    }

    async fn list_devices(&self, user_id: UserId) -> RepoResult<Vec<DeviceKey>> {
        let models = device_key::Entity::find()
            .filter(device_key::Column::UserId.eq(Uuid::from(user_id)))
            .order_by_asc(device_key::Column::AddedAt)
            .order_by_asc(device_key::Column::Id)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        models.into_iter().map(TryInto::try_into).collect()
    }

    async fn revoke_device(
        &self,
        id: DeviceKeyId,
        revocation_payload: &str,
        revocation_signature: &str,
    ) -> RepoResult<bool> {
        let res = device_key::Entity::update_many()
            .col_expr(device_key::Column::RevokedAt, Expr::value(Utc::now()))
            .col_expr(
                device_key::Column::RevocationPayload,
                Expr::value(revocation_payload),
            )
            .col_expr(
                device_key::Column::RevocationSignature,
                Expr::value(revocation_signature),
            )
            .filter(device_key::Column::Id.eq(Uuid::from(id)))
            .filter(device_key::Column::RevokedAt.is_null())
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected == 1)
    }

    async fn revoke_all_devices(&self, user_id: UserId) -> RepoResult<u64> {
        let res = device_key::Entity::update_many()
            .col_expr(device_key::Column::RevokedAt, Expr::value(Utc::now()))
            .filter(device_key::Column::UserId.eq(Uuid::from(user_id)))
            .filter(device_key::Column::RevokedAt.is_null())
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }

    async fn touch_device(&self, id: DeviceKeyId) -> RepoResult<()> {
        device_key::Entity::update_many()
            .col_expr(device_key::Column::LastUsedAt, Expr::value(Utc::now()))
            .filter(device_key::Column::Id.eq(Uuid::from(id)))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn get_profile_card(&self, user_id: UserId) -> RepoResult<Option<StoredProfileCard>> {
        let model = profile_card::Entity::find_by_id(Uuid::from(user_id))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        model.map(TryInto::try_into).transpose()
    }

    async fn upsert_profile_card(
        &self,
        user_id: UserId,
        version: u32,
        payload: &str,
        signature: &str,
    ) -> RepoResult<bool> {
        let user_uuid: Uuid = user_id.into();
        // One statement, so two publishers racing past a version check can't both win: the
        // update only fires when the new version is greater.
        let res = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"INSERT INTO profile_card (user_id, version, payload, signature, updated_at)
                   VALUES ($1, $2, $3, $4, now())
                   ON CONFLICT (user_id) DO UPDATE
                       SET version = EXCLUDED.version,
                           payload = EXCLUDED.payload,
                           signature = EXCLUDED.signature,
                           updated_at = EXCLUDED.updated_at
                       WHERE profile_card.version < EXCLUDED.version"#,
                [
                    user_uuid.into(),
                    i64::from(version).into(),
                    payload.into(),
                    signature.into(),
                ],
            ))
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected() == 1)
    }
}

pub struct SeaOrmKeyChallengeRepository {
    db: DbHandle,
}

impl SeaOrmKeyChallengeRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl KeyChallengeRepository for SeaOrmKeyChallengeRepository {
    async fn create(&self, nonce: &str, expires_at: DateTime<Utc>) -> RepoResult<KeyChallenge> {
        let model = key_challenge::ActiveModel {
            id: Set(KeyChallengeId::new().into()),
            nonce: Set(nonce.to_string()),
            expires_at: Set(expires_at.into()),
            ..Default::default()
        };
        let res = key_challenge::Entity::insert(model)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.into())
    }

    async fn consume(&self, id: KeyChallengeId, nonce: &str) -> RepoResult<bool> {
        let now = Utc::now();
        let res = key_challenge::Entity::update_many()
            .col_expr(key_challenge::Column::UsedAt, Expr::value(now))
            .filter(key_challenge::Column::Id.eq(Uuid::from(id)))
            .filter(key_challenge::Column::Nonce.eq(nonce))
            .filter(key_challenge::Column::UsedAt.is_null())
            .filter(key_challenge::Column::ExpiresAt.gt(now))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected == 1)
    }

    async fn delete_expired(&self, before: DateTime<Utc>) -> RepoResult<u64> {
        let res = key_challenge::Entity::delete_many()
            .filter(key_challenge::Column::ExpiresAt.lt(before))
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(res.rows_affected)
    }
}
