use rim_identity::IdentityError;

/// Why an HTTP message signature was refused. [`SignatureRefusal::as_str`] is a stable reason
/// for logs and metrics; none of them carries key material or payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SignatureRefusal {
    #[error("a required header is missing")]
    MissingHeader,
    #[error("Signature-Input or Signature is malformed")]
    Malformed,
    #[error("no signature with the expected label")]
    MissingLabel,
    #[error("the covered components are not the profile's")]
    Components,
    #[error("the signature parameters are not the profile's")]
    Parameters,
    #[error("the signature is outside its time window")]
    Clock,
    #[error("the keyid is malformed")]
    KeyId,
    #[error("the signature algorithm is not the key's")]
    Algorithm,
    #[error("Content-Digest is missing or does not match the body")]
    Digest,
    #[error("the response answers another request")]
    RequestNonce,
    #[error("the signature does not verify")]
    Verify,
}

impl SignatureRefusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingHeader => "missing_header",
            Self::Malformed => "malformed",
            Self::MissingLabel => "missing_label",
            Self::Components => "components",
            Self::Parameters => "parameters",
            Self::Clock => "clock",
            Self::KeyId => "keyid",
            Self::Algorithm => "algorithm",
            Self::Digest => "digest",
            Self::RequestNonce => "request_nonce",
            Self::Verify => "verify",
        }
    }
}

/// Why a server key log, descriptor or server-signed statement was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StatementError {
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error("{0}")]
    Invalid(String),
}

impl StatementError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

pub type StatementResult<T> = Result<T, StatementError>;
