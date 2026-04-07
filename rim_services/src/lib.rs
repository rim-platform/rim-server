pub mod action;
pub mod auth;
pub mod capability;
pub mod channel_lens;
pub mod command;
pub mod conversation;
pub mod custom_emoji;
pub mod event;
pub mod events;
pub mod incident;
pub mod integration;
pub mod keybind;
pub mod link_preview;
pub mod macro_def;
pub mod mention;
pub mod message;
pub mod message_attachment;
pub mod message_reaction;
pub mod message_thread;
pub mod notification_preference;
pub mod password_reset;
pub mod pin;
pub mod plugin_host;
pub mod presence;
pub mod provider;
pub mod read_state;
pub mod server;
pub mod server_member;
pub mod setting;
pub mod skill_profile;
pub mod storage;
pub mod user;

use rim_core::principal::model::Principal;
use rim_core::repository::error::{RepoError, RepoErrorKind};
use rim_core::user::model::UserId;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

    #[error("internal error")]
    Internal(#[source] Option<anyhow::Error>),
}

pub type ServiceResult<T> = Result<T, ServiceError>;

impl ServiceError {
    pub fn internal<E>(err: E) -> Self
    where
        E: Into<anyhow::Error>,
    {
        Self::Internal(Some(err.into()))
    }
}

pub fn map_repo_error(err: RepoError) -> ServiceError {
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

#[cfg(test)]
mod tests;
