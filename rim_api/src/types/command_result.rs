use async_graphql::{Enum, InputObject, SimpleObject};

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

/// GraphQL input for submitting a confirmation proof: the token and args
/// hash of the challenge a previous call returned.
#[derive(InputObject, Clone, Debug)]
pub struct ConfirmationProofInput {
    pub in_band_token: InBandTokenInput,
}

#[derive(InputObject, Clone, Debug)]
pub struct InBandTokenInput {
    pub token: String,
    pub args_hash: String,
}

impl ConfirmationProofInput {
    /// The service-layer [`rim_services::command::confirmation::ConfirmationProof`].
    pub fn into_proof(self) -> rim_services::command::confirmation::ConfirmationProof {
        rim_services::command::confirmation::ConfirmationProof::InBandToken {
            token: self.in_band_token.token,
            args_hash: self.in_band_token.args_hash,
        }
    }
}
