pub mod alert;
pub mod audit;
pub mod auth;
pub mod capability;
pub mod channel_lens;
pub mod command;
pub mod conversation;
pub mod custom_emoji;
pub mod device_link;
pub mod e2ee;
pub mod escalation;
pub mod events;
pub mod incident;
pub mod ingest_token;
pub mod key_auth;
pub mod keybind;
pub mod link_preview;
pub mod macro_def;
pub mod mention;
pub mod message;
pub mod message_attachment;
pub mod message_reaction;
pub mod message_thread;
pub mod moderation;
pub mod notification;
pub mod notification_preference;
pub mod oncall;
pub mod password_reset;
pub mod pin;
pub mod pipeline;
pub mod presence;
pub mod provider;
pub mod read_state;
pub mod server;
pub mod server_member;
pub mod skill_profile;
pub mod storage;
pub mod typing;
pub mod user;
pub mod workspace_feature;
pub mod workspace_plugin;
pub mod workspace_state;

use rim_core::principal::model::Principal;
use rim_core::repository::error::{RepoError, RepoErrorKind};
use rim_core::user::model::UserId;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Actor {
    pub principal: Principal,
    pub user_id: UserId,
}

impl Actor {
    pub fn new(user_id: UserId) -> Self {
        Self {
            principal: Principal::user(user_id),
            user_id,
        }
    }

    pub fn from_principal(principal: Principal) -> Option<Self> {
        principal.user_id.map(|user_id| Self { principal, user_id })
    }
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("not found")]
    NotFound,

    #[error("validation error: {0}")]
    Validation(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("dependency unavailable")]
    Unavailable,

    #[error("dependency timed out")]
    Timeout,

    #[error("permission denied")]
    PermissionDenied,

    #[error("unauthenticated")]
    Unauthenticated,

    /// The submitted [`crate::command::confirmation::ConfirmationProof`]
    /// was unknown, expired, single-use-consumed, or args-mismatched.
    /// Caller should re-issue the dangerous command (server will return
    /// a fresh challenge), present the modal again, and retry. Used by
    /// the H1 (TOCTOU) gate.
    #[error("confirmation proof is stale; please re-confirm")]
    ConfirmationStale,

    /// The submitted [`crate::command::confirmation::ConfirmationProof`]
    /// failed verification in a way that indicates misuse (wrong command
    /// id, wrong context, wrong actor) or an unsupported proof variant.
    /// The detail string is non-actionable for the user; surface a
    /// generic "confirmation failed" message.
    #[error("confirmation failed: {0}")]
    ConfirmationFailed(String),

    /// The caller has exceeded the per-(actor, command_id) rate limit.
    /// `retry_after_seconds` is how long to wait before the oldest token
    /// in the window expires and one slot opens up again.
    #[error("rate limit exceeded; retry after {retry_after_seconds}s")]
    RateLimited { retry_after_seconds: u64 },

    /// The presented registration token was not found, has already been used,
    /// or has been revoked. Deliberately indistinguishable across all three
    /// conditions so an attacker cannot probe token state (§4.5).
    #[error("invalid or unknown token")]
    InvalidToken,

    /// The presented registration token exists but its `expires_at` is in the
    /// past. Returning a distinct code here is acceptable — expiry is not
    /// exploitable and the FE needs to surface a "link expired" message (§4.5).
    #[error("token has expired")]
    ExpiredToken,

    /// The signup email address is already associated with an existing user
    /// (case-insensitive comparison). Deliberately distinguishable from the
    /// generic `CONFLICT` code so the frontend can highlight the email field
    /// specifically (spec §13.5).
    #[error("email already in use")]
    EmailTaken,

    /// The workspace has the named optional module turned off (Track 0.4). `feature` is the
    /// stable snake_case name (`rim_core::workspace_feature::model::WorkspaceFeature::as_str`).
    /// Surfaced as the `FEATURE_DISABLED` GraphQL error code.
    #[error("workspace feature `{feature}` is disabled")]
    FeatureDisabled { feature: String },

    /// The conversation is archived and therefore read-only (Track 1.1). Returned by every
    /// write path into a conversation (messages, reactions, pins, topic, attachments, typing,
    /// incidents). Surfaced as the `CONVERSATION_ARCHIVED` GraphQL error code.
    #[error("conversation is archived")]
    ConversationArchived,

    /// The actor is timed out in this workspace until `until` (Track 1.2): writes (messages,
    /// reactions, pins, topic, attachments, joins, invites, conversation creation and Mutating
    /// commands) are refused while reads continue. Surfaced as the `TIMED_OUT` GraphQL error
    /// code with an `until` extension.
    #[error("timed out until {until}")]
    TimedOut {
        until: chrono::DateTime<chrono::Utc>,
    },

    /// Refused with a reason the caller may see, as `FORBIDDEN`.
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// An MLS Commit or message for an epoch or incarnation that is not the conversation's
    /// current one. Carries the current ones, so the client knows what to catch up to.
    #[error("stale MLS epoch; the group is at incarnation {incarnation}, epoch {epoch}")]
    StaleEpoch { incarnation: i32, epoch: i64 },

    /// An encrypted message needs a Commit first.
    #[error("an MLS Commit is required first: {}", reason.as_str())]
    MlsCommitRequired {
        reason: crate::e2ee::MlsCommitRequiredReason,
    },

    #[error("internal error")]
    Internal(#[source] Option<anyhow::Error>),
}

pub type ServiceResult<T> = Result<T, ServiceError>;

impl ServiceError {
    /// Shorthand for [`ServiceError::FeatureDisabled`] from a typed feature.
    pub fn feature_disabled(feature: rim_core::workspace_feature::model::WorkspaceFeature) -> Self {
        Self::FeatureDisabled {
            feature: feature.as_str().to_string(),
        }
    }

    pub fn internal<E>(err: E) -> Self
    where
        E: Into<anyhow::Error>,
    {
        Self::Internal(Some(err.into()))
    }
}

impl From<RepoError> for ServiceError {
    fn from(err: RepoError) -> Self {
        match err.kind() {
            RepoErrorKind::NotFound => ServiceError::NotFound,
            RepoErrorKind::Conflict => {
                ServiceError::Conflict(err.message().unwrap_or("conflict").to_string())
            }
            RepoErrorKind::Timeout => ServiceError::Timeout,
            RepoErrorKind::Unavailable => ServiceError::Unavailable,
            RepoErrorKind::InvalidData | RepoErrorKind::Internal => ServiceError::internal(err),
            RepoErrorKind::Unsupported => ServiceError::internal(err),
            RepoErrorKind::Canceled => ServiceError::internal(err),
            RepoErrorKind::PreconditionFailed => {
                ServiceError::Conflict("precondition failed".to_string())
            }
            _ => ServiceError::internal(err),
        }
    }
}

#[cfg(test)]
mod tests;
