pub mod csrf;
pub mod password;
pub mod session_token;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,

    #[error("password hashing error")]
    PasswordHash,
}

pub type AuthResult<T> = Result<T, AuthError>;
