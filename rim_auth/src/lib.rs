pub mod password;
pub mod session_token;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,

    #[error("invalid session")]
    InvalidSession,

    #[error("session expired")]
    SessionExpired,

    #[error("password hashing error")]
    PasswordHash,
}

pub type AuthResult<T> = Result<T, AuthError>;
