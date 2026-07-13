use rim_core::conversation::model::ConversationId;
use rim_core::escalation::model::EscalationPolicyId;
use rim_core::server::model::ServerId;

use crate::Actor;
use crate::escalation::dto::EscalationStepInput;

#[derive(Debug, Clone)]
pub struct CreateEscalationPolicyCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub name: String,
    /// Full ordered step list — position is assigned from array order (§3). At least one step,
    /// each with at least one action, is required.
    pub steps: Vec<EscalationStepInput>,
}

/// Updates a policy's `name` and/or its full step list. `steps: Some(_)` replaces every step
/// atomically (delete-then-insert, mirroring `setRotationParticipants`); `None` leaves the
/// existing steps untouched.
#[derive(Debug, Clone)]
pub struct UpdateEscalationPolicyCmd {
    pub actor: Actor,
    pub policy_id: EscalationPolicyId,
    pub name: Option<String>,
    pub steps: Option<Vec<EscalationStepInput>>,
}

#[derive(Debug, Clone)]
pub struct DeleteEscalationPolicyCmd {
    pub actor: Actor,
    pub policy_id: EscalationPolicyId,
}

/// `setChannelEscalationPolicy(conversationId, policyId)` — sets the default policy applied to
/// incidents declared in this conversation (`ingest_alert`'s only source of a policy, since
/// there's no human to pick an override; `start_incident` may still override it per-call).
#[derive(Debug, Clone)]
pub struct SetChannelEscalationPolicyCmd {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub policy_id: EscalationPolicyId,
}

#[derive(Debug, Clone)]
pub struct ListEscalationPoliciesQuery {
    pub actor: Actor,
    pub server_id: ServerId,
}
