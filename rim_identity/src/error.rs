/// Why a signed statement was refused.
///
/// The messages are safe to show to the client that sent the statement: they name the rule that
/// failed, never key material.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error("statement payload exceeds {max} bytes")]
    TooLarge { max: usize },

    /// The payload is not JSON of the statement's shape. The parser's description is kept for
    /// the log ([`IdentityError::detail`]) and never shown: it would echo parser internals.
    #[error("statement payload is not valid")]
    Malformed(String),

    #[error("unsupported statement version {0}")]
    UnsupportedVersion(u32),

    #[error("expected a `{expected}` statement")]
    WrongType { expected: &'static str },

    #[error("{0} is not in its canonical encoding")]
    BadEncoding(&'static str),

    #[error("{0} names an unsupported algorithm `{1}`")]
    UnsupportedAlgorithm(&'static str, String),

    #[error("{0} is a weak or non-canonical Ed25519 key")]
    WeakKey(&'static str),

    #[error("signature algorithm does not match the signing key")]
    AlgorithmMismatch,

    #[error("signature does not verify")]
    BadSignature,

    #[error("a version {got} statement cannot follow a version {head} key log entry")]
    VersionDowngrade { head: u32, got: u32 },

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

impl IdentityError {
    /// The parser's description of a [`IdentityError::Malformed`] payload, for the log.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Malformed(detail) => Some(detail),
            _ => None,
        }
    }
}

pub type IdentityResult<T> = Result<T, IdentityError>;
