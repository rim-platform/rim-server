pub mod conversation;
pub mod user;
pub mod server;
pub mod server_member;
pub mod message_thread;
pub mod message;
pub mod message_attachment;

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

    #[error("permission denied")]
    PermissionDenied,

    #[error("internal error")]
    Internal,
}

pub type ServiceResult<T> = Result<T, ServiceError>;
