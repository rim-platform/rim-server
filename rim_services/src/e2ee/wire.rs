//! The seam between the services and the MLS library: decoding the framing of MLS messages
//! and validating key packages. The server never decrypts and never holds group state.

#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

/// Why bytes were refused; the text is safe to show the client that sent them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct MlsWireError(pub String);

/// A key package that decoded and passed the library's validation (signatures, protocol
/// version, lifetime).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPackageInfo {
    pub cipher_suite: i32,
    /// The library's `KeyPackageRef` (`hash_ref`).
    pub key_package_ref: Vec<u8>,
    /// The basic credential's identity bytes.
    pub credential_identity: Vec<u8>,
    /// The leaf's signature key, raw.
    pub signature_key: Vec<u8>,
    /// Seconds since the Unix epoch.
    pub not_after: u64,
    pub last_resort: bool,
}

/// The content type of a PrivateMessage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlsContentType {
    Application,
    Proposal,
    Commit,
}

/// The cleartext header of a PrivateMessage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateMessageInfo {
    pub group_id: Vec<u8>,
    pub epoch: u64,
    pub content_type: MlsContentType,
    pub authenticated_data: Vec<u8>,
}

/// What a Welcome says about itself without being opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WelcomeInfo {
    pub cipher_suite: i32,
    /// The `KeyPackageRef`s of the new members the Welcome is encrypted to.
    pub new_member_refs: Vec<Vec<u8>>,
}

/// Decoding of RFC 9420 `MLSMessage` bytes.
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MlsWire: Send + Sync {
    /// Decode an `MLSMessage` carrying a key package and validate it. Refuses any other
    /// credential type than basic.
    fn read_key_package(&self, bytes: &[u8]) -> Result<KeyPackageInfo, MlsWireError>;

    /// Decode an `MLSMessage` carrying a PrivateMessage and read its header.
    fn read_private_message(&self, bytes: &[u8]) -> Result<PrivateMessageInfo, MlsWireError>;

    /// Decode an `MLSMessage` carrying a Welcome.
    fn read_welcome(&self, bytes: &[u8]) -> Result<WelcomeInfo, MlsWireError>;
}
