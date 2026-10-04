use async_trait::async_trait;
use entity::escalation_step;
use rim_core::escalation::model::{
    EscalationPolicyId, EscalationStep, EscalationStepId, EscalationTarget, NewEscalationStep,
    encode_actions,
};
use rim_core::escalation::repository::EscalationStepRepository;
use rim_core::repository::error::RepoResult;
use sea_orm::prelude::Uuid;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmEscalationStepRepository {
    pub db: DbHandle,
}

impl SeaOrmEscalationStepRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl EscalationStepRepository for SeaOrmEscalationStepRepository {
    async fn list_by_policy(
        &self,
        policy_id: EscalationPolicyId,
    ) -> RepoResult<Vec<EscalationStep>> {
        let models = escalation_step::Entity::find()
            .filter(escalation_step::Column::PolicyId.eq::<Uuid>(policy_id.into()))
            .order_by_asc(escalation_step::Column::Position)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn set_steps(
        &self,
        policy_id: EscalationPolicyId,
        steps: Vec<NewEscalationStep>,
    ) -> RepoResult<Vec<EscalationStep>> {
        let policy_uuid: Uuid = policy_id.into();
        let new_rows: Vec<(Uuid, i32, i64, String, Option<Uuid>, Option<Uuid>, String)> = steps
            .into_iter()
            .map(|step| {
                let (target_kind, target_user_id, target_schedule_id) = match step.target {
                    EscalationTarget::User(uid) => ("user".to_string(), Some(uid.into()), None),
                    EscalationTarget::OnCall(sid) => {
                        ("on_call".to_string(), None, Some(sid.into()))
                    }
                };
                (
                    Uuid::from(EscalationStepId::new()),
                    step.position,
                    step.delay_seconds,
                    target_kind,
                    target_user_id,
                    target_schedule_id,
                    encode_actions(&step.actions),
                )
            })
            .collect();

        // Delete-then-insert must be atomic — mirrors
        // `SeaOrmRotationParticipantRepository::set_participants`.
        self.db
            .transaction::<_, (), sea_orm::DbErr>(move |txn| {
                Box::pin(async move {
                    escalation_step::Entity::delete_many()
                        .filter(escalation_step::Column::PolicyId.eq(policy_uuid))
                        .exec(txn)
                        .await?;

                    if !new_rows.is_empty() {
                        let actives = new_rows.into_iter().map(
                            |(
                                id,
                                position,
                                delay_seconds,
                                target_kind,
                                target_user_id,
                                target_schedule_id,
                                actions,
                            )| {
                                escalation_step::ActiveModel {
                                    id: Set(id),
                                    policy_id: Set(policy_uuid),
                                    position: Set(position),
                                    delay_seconds: Set(delay_seconds),
                                    target_kind: Set(target_kind),
                                    target_user_id: Set(target_user_id),
                                    target_schedule_id: Set(target_schedule_id),
                                    actions: Set(actions),
                                }
                            },
                        );
                        escalation_step::Entity::insert_many(actives)
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

        self.list_by_policy(policy_id).await
    }
}
