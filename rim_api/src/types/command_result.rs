use async_graphql::{Enum, InputObject, Result, SimpleObject};

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CommandResultStatus {
    Success,
    Error,
    /// Command is `dangerous: true` and the caller did not submit a
    /// matching [`ConfirmationProofInput`]. The accompanying
    /// `confirmationRequired` field on the result carries the
    /// server-issued challenge — present the modal, then re-submit with
    /// `confirmation: { inBandToken: { token, argsHash } }`.
    ConfirmationRequired,
}

impl From<rim_services::command::dto::CommandResultStatusDto> for CommandResultStatus {
    fn from(value: rim_services::command::dto::CommandResultStatusDto) -> Self {
        use rim_services::command::dto::CommandResultStatusDto as D;
        match value {
            D::Success => Self::Success,
            D::Error => Self::Error,
            D::ConfirmationRequired => Self::ConfirmationRequired,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PipeStageResult {
    pub stage_index: u32,
    pub command_id: String,
    pub status: CommandResultStatus,
    pub message: String,
    pub payload: Option<async_graphql::Json<serde_json::Value>>,
}

impl From<rim_services::command::dto::PipeStageResultDto> for PipeStageResult {
    fn from(value: rim_services::command::dto::PipeStageResultDto) -> Self {
        Self {
            stage_index: value.stage_index,
            command_id: value.command_id,
            status: value.status.into(),
            message: value.message,
            payload: value.payload.map(async_graphql::Json),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct CommandResult {
    pub status: CommandResultStatus,
    pub message: String,
    pub payload: Option<async_graphql::Json<serde_json::Value>>,
    pub follow_up_commands: Vec<String>,
    pub pipeline_stages: Option<Vec<PipeStageResult>>,
    /// Populated **iff** `status == CONFIRMATION_REQUIRED`. Carries the
    /// server-issued challenge (token + argsHash + expiresAt). The
    /// frontend presents the modal, then re-issues the original
    /// `executeCommand` mutation with `confirmation: { inBandToken: { ... } }`.
    pub confirmation_required: Option<ConfirmationChallenge>,
}

impl From<rim_services::command::dto::CommandResultDto> for CommandResult {
    fn from(value: rim_services::command::dto::CommandResultDto) -> Self {
        Self {
            status: value.status.into(),
            message: value.message,
            payload: value.payload.map(async_graphql::Json),
            follow_up_commands: value.follow_up_commands,
            pipeline_stages: value
                .pipeline_stages
                .map(|stages| stages.into_iter().map(Into::into).collect()),
            confirmation_required: value.confirmation_required.map(Into::into),
        }
    }
}

/// GraphQL projection of a server-issued
/// [`rim_services::command::confirmation::ConfirmationChallengeDto`]. The
/// frontend opens its confirm-modal with these fields, then submits a
/// matching [`ConfirmationProofInput`].
#[derive(SimpleObject, Clone, Debug)]
pub struct ConfirmationChallenge {
    pub token: String,
    pub args_hash: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::command::confirmation::ConfirmationChallengeDto> for ConfirmationChallenge {
    fn from(value: rim_services::command::confirmation::ConfirmationChallengeDto) -> Self {
        Self {
            token: value.token,
            args_hash: value.args_hash,
            expires_at: value.expires_at,
        }
    }
}

/// GraphQL input for submitting a confirmation proof. Exactly one of
/// `inBandToken` or `outOfBand` MUST be populated; submitting both or
/// neither results in a `VALIDATION` error.
///
/// `outOfBand` is a Phase-1 placeholder for MCP-style consent flows
/// (M5 / Phase 6); the server currently rejects it with
/// `CONFIRMATION_FAILED { code: UNSUPPORTED }`.
#[derive(InputObject, Clone, Debug)]
pub struct ConfirmationProofInput {
    pub in_band_token: Option<InBandTokenInput>,
    pub out_of_band: Option<OutOfBandProofInput>,
}

#[derive(InputObject, Clone, Debug)]
pub struct InBandTokenInput {
    pub token: String,
    pub args_hash: String,
}

#[derive(InputObject, Clone, Debug)]
pub struct OutOfBandProofInput {
    pub request_id: String,
}

impl ConfirmationProofInput {
    /// Convert the GraphQL input into the service-layer
    /// [`rim_services::command::confirmation::ConfirmationProof`].
    ///
    /// **Oneof discipline (Widow §W3 closure):** if a caller submits
    /// `confirmation: { ... }` they MUST populate exactly one variant.
    /// Both populated → `VALIDATION` error. Both `None` → also a
    /// `VALIDATION` error (caller should omit the `confirmation` field
    /// entirely if they have no proof to submit; an empty proof object
    /// is treated as a malformed request, not as "no proof", so an
    /// attacker stamping `confirmation: {}` cannot silently burn a
    /// token slot per request).
    pub fn into_proof(
        self,
    ) -> Result<rim_services::command::confirmation::ConfirmationProof, async_graphql::Error> {
        use rim_services::command::confirmation::ConfirmationProof as P;
        match (self.in_band_token, self.out_of_band) {
            (Some(_), Some(_)) => Err(async_graphql::Error::new(
                "ConfirmationProofInput must populate exactly one of inBandToken / outOfBand",
            )),
            (None, None) => Err(async_graphql::Error::new(
                "ConfirmationProofInput must populate exactly one of inBandToken / outOfBand; \
                 omit the field entirely if you have no proof to submit",
            )),
            (Some(t), None) => Ok(P::InBandToken {
                token: t.token,
                args_hash: t.args_hash,
            }),
            (None, Some(o)) => Ok(P::OutOfBand {
                request_id: o.request_id,
            }),
        }
    }
}
