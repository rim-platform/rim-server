use async_trait::async_trait;
use entity::conversation_escalation_policy;
use rim_core::conversation::model::ConversationId;
use rim_core::escalation::model::EscalationPolicyId;
use rim_core::escalation::repository::ConversationEscalationPolicyRepository;
use rim_core::repository::error::RepoResult;
use rim_core::server::model::ServerId;
use sea_orm::prelude::Uuid;
use sea_orm::sea_query::OnConflict;
use sea_orm::{EntityTrait, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmConversationEscalationPolicyRepository {
    pub db: DbHandle,
}

impl SeaOrmConversationEscalationPolicyRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ConversationEscalationPolicyRepository for SeaOrmConversationEscalationPolicyRepository {
    async fn set(
        &self,
        conversation_id: ConversationId,
        server_id: ServerId,
        policy_id: EscalationPolicyId,
    ) -> RepoResult<()> {
        let active = conversation_escalation_policy::ActiveModel {
            conversation_id: Set(conversation_id.into()),
            server_id: Set(server_id.into()),
            policy_id: Set(policy_id.into()),
        };
        // Upsert via `ON CONFLICT` rather than find-then-save: a conversation may already have a
        // binding row (re-setting its default policy), and this avoids a TOCTOU race between the
        // lookup and the write under concurrent calls.
        conversation_escalation_policy::Entity::insert(active)
            .on_conflict(
                OnConflict::column(conversation_escalation_policy::Column::ConversationId)
                    .update_column(conversation_escalation_policy::Column::PolicyId)
                    .to_owned(),
            )
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }

    async fn get(&self, conversation_id: ConversationId) -> RepoResult<Option<EscalationPolicyId>> {
        let model =
            conversation_escalation_policy::Entity::find_by_id::<Uuid>(conversation_id.into())
                .one(&self.db)
                .await
                .map_err(map_db_err)?;
        Ok(model.map(|m| m.policy_id.into()))
    }
}
