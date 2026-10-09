/// Why a signed statement was refused.
///
/// The messages are safe to show to the client that sent the statement: they name the rule that
/// failed, never key material.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error("statement payload exceeds {max} bytes")]
    TooLarge { max: usize },

    #[error("statement payload is not valid: {0}")]
    Malformed(String),

    #[error("unsupported statement version {0}")]
    UnsupportedVersion(u32),

    #[error("expected a `{expected}` statement")]
    WrongType { expected: &'static str },

    #[error("{0} is not valid unpadded base64url of the right length")]
    BadEncoding(&'static str),

    #[error("{0} is a weak or non-canonical Ed25519 key")]
    WeakKey(&'static str),

    #[error("signature does not verify")]
    BadSignature,

    #[error("statement is for a different principal")]
    PrincipalMismatch,

    #[error("key log entry has seq {got}, expected {expected}")]
    SequenceMismatch { expected: u32, got: u32 },

    #[error("key log entry does not chain to the previous entry")]
    ChainMismatch,

    #[error("statement was signed under key log seq {got}, the current seq is {current}")]
    StaleKey { current: u32, got: u32 },

    #[error("{0}")]
    Invalid(String),
}

pub type IdentityResult<T> = Result<T, IdentityError>;
