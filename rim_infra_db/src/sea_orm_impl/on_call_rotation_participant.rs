use async_trait::async_trait;
use entity::on_call_rotation_participant;
use rim_core::oncall::model::{RotationLayerId, RotationParticipant, RotationParticipantId};
use rim_core::oncall::repository::RotationParticipantRepository;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmRotationParticipantRepository {
    pub db: DbHandle,
}

impl SeaOrmRotationParticipantRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl RotationParticipantRepository for SeaOrmRotationParticipantRepository {
    async fn list_by_layer(
        &self,
        layer_id: RotationLayerId,
    ) -> RepoResult<Vec<RotationParticipant>> {
        let models = on_call_rotation_participant::Entity::find()
            .filter(on_call_rotation_participant::Column::LayerId.eq::<Uuid>(layer_id.into()))
            .order_by_asc(on_call_rotation_participant::Column::Position)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn set_participants(
        &self,
        layer_id: RotationLayerId,
        user_ids: &[UserId],
    ) -> RepoResult<Vec<RotationParticipant>> {
        let layer_uuid: Uuid = layer_id.into();
        let new_rows: Vec<(Uuid, Uuid, i32)> = user_ids
            .iter()
            .enumerate()
            .map(|(position, user_id)| {
                (
                    Uuid::from(RotationParticipantId::new()),
                    Uuid::from(*user_id),
                    position as i32,
                )
            })
            .collect();

        // Delete-then-insert must be atomic: a crash between the two would leave the layer with
        // no participants. `DbHandle::transaction` uses a savepoint if we're already inside a
        // service-level transaction, or opens a fresh one otherwise.
        self.db
            .transaction::<_, (), sea_orm::DbErr>(move |txn| {
                Box::pin(async move {
                    on_call_rotation_participant::Entity::delete_many()
                        .filter(on_call_rotation_participant::Column::LayerId.eq(layer_uuid))
                        .exec(txn)
                        .await?;

                    if !new_rows.is_empty() {
                        let actives = new_rows.into_iter().map(|(id, user_id, position)| {
                            on_call_rotation_participant::ActiveModel {
                                id: Set(id),
                                layer_id: Set(layer_uuid),
                                user_id: Set(user_id),
                                position: Set(position),
                            }
                        });
                        on_call_rotation_participant::Entity::insert_many(actives)
                            .exec(txn)
                            .await?;
                    }

                    Ok(())
                })
            })
            .await
            .map_err(|e| match e {
                sea_orm::TransactionError::Connection(db_err) => map_db_err(db_err),
                sea_orm::TransactionError::Transaction(db_err) => map_db_err(db_err),
            })?;

        self.list_by_layer(layer_id).await
    }
}
