use std::error::Error as StdError;
use std::fmt;

pub type RepoResult<T> = Result<T, RepoError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RepoErrorKind {
    NotFound,
    Conflict,
    PreconditionFailed,
    InvalidData,
    Timeout,
    Unavailable,
    Canceled,
    Unsupported,
    Internal,
}

#[derive(Debug)]
pub struct RepoError {
    kind: RepoErrorKind,
    message: Option<String>,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl RepoError {
    pub fn new(kind: RepoErrorKind) -> Self {
        Self {
            kind,
            message: None,
            source: None,
        }
    }

    pub fn with_message(kind: RepoErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: Some(message.into()),
            source: None,
        }
    }

    pub fn with_source(kind: RepoErrorKind, source: impl StdError + Send + Sync + 'static) -> Self {
        Self {
            kind,
            message: None,
            source: Some(Box::new(source)),
        }
    }

    pub fn with_message_and_source(
        kind: RepoErrorKind,
        message: impl Into<String>,
        source: impl StdError + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind,
            message: Some(message.into()),
            source: Some(Box::new(source)),
        }
    }

    pub fn kind(&self) -> RepoErrorKind {
        self.kind
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

impl fmt::Display for RepoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.kind, self.message.as_deref()) {
            (kind, Some(msg)) => write!(f, "{kind:?}: {msg}"),
            (kind, None) => write!(f, "{kind:?}"),
        }
    }
}

impl StdError for RepoError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source.as_deref().map(|e| e as _)
    }
}
