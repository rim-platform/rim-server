pub mod auth;
pub mod conversation;
pub mod message;
pub mod message_attachment;
pub mod message_thread;
pub mod server;
pub mod server_member;
pub mod user;

use rim_core::repository::error::{RepoError, RepoErrorKind};
use rim_core::user::model::UserId;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Actor {
    pub user_id: UserId,
}

impl Actor {
    pub fn new(user_id: UserId) -> Self {
        Self { user_id }
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
