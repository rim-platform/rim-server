//! End-to-end encryption of DMs and groups with MLS: the server stores key packages, sequences
//! each conversation's group (one Commit per epoch), delivers Welcomes, and keeps encrypted
//! messages as opaque envelopes it reads only the framing of.

pub mod command;
pub mod dto;
pub mod service;
pub mod wire;

/// MLS cipher suites the server accepts key packages and groups for.
pub const ACCEPTED_CIPHER_SUITES: &[i32] = &[PREFERRED_CIPHER_SUITE];

/// `MLS_128_MLKEM768X25519_AES128GCM_SHA256_Ed25519`: X25519 + ML-KEM-768 key agreement,
/// AES-128-GCM, Ed25519 signatures.
pub const PREFERRED_CIPHER_SUITE: i32 = 0x004f;

/// Largest key package, in bytes.
pub const MAX_KEY_PACKAGE_BYTES: usize = 16 * 1024;
/// Most key packages per upload.
pub const MAX_KEY_PACKAGES_PER_UPLOAD: usize = 50;
/// Most unclaimed one-time key packages per device and suite.
pub const MAX_AVAILABLE_KEY_PACKAGES: i64 = 100;
/// Largest Commit and Welcome, in bytes.
pub const MAX_COMMIT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_WELCOME_BYTES: usize = 4 * 1024 * 1024;
/// Largest authenticated data of a Commit, in bytes.
pub const MAX_COMMIT_AAD_BYTES: usize = 256 * 1024;
/// Largest application message envelope, in bytes.
pub const MAX_ENVELOPE_BYTES: usize = 96 * 1024;
/// Largest authenticated data of an application message, in bytes.
pub const MAX_APPLICATION_AAD_BYTES: usize = 8 * 1024;
/// Largest decrypted content a report may reveal, in bytes.
pub const MAX_CONTENT_BYTES: usize = 64 * 1024;
/// Most members and devices of an encrypted conversation.
pub const MAX_ENCRYPTED_MEMBERS: usize = 100;
pub const MAX_GROUP_DEVICES: usize = 512;
/// Days without a key package upload after which a device is no longer MLS-active.
pub const MLS_ACTIVE_DAYS: i64 = 60;

/// Why a message needs a Commit first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlsCommitRequiredReason {
    /// Devices that left the conversation are still in the group.
    PendingRemovals,
    /// The encrypted conversation has no group yet.
    NoGroup,
}

impl MlsCommitRequiredReason {
    /// The value of the error's `reason` extension.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingRemovals => "pendingRemovals",
            Self::NoGroup => "noGroup",
        }
    }
}

/// The MLS group id of a conversation's incarnation: ASCII `rim1`, the conversation UUID's 16
/// bytes, and the incarnation as a big-endian u32.
pub fn mls_group_id(
    conversation_id: rim_core::conversation::model::ConversationId,
    incarnation: u32,
) -> Vec<u8> {
    let uuid: uuid::Uuid = conversation_id.into();
    let mut id = Vec::with_capacity(24);
    id.extend_from_slice(b"rim1");
    id.extend_from_slice(uuid.as_bytes());
    id.extend_from_slice(&incarnation.to_be_bytes());
    id
}

/// The conversation and incarnation a group id names, if it has the shape of one.
pub fn parse_mls_group_id(
    group_id: &[u8],
) -> Option<(rim_core::conversation::model::ConversationId, u32)> {
    let rest = group_id.strip_prefix(b"rim1")?;
    if rest.len() != 20 {
        return None;
    }
    let uuid = uuid::Uuid::from_slice(&rest[..16]).ok()?;
    let incarnation = u32::from_be_bytes(rest[16..].try_into().ok()?);
    Some((uuid.into(), incarnation))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_id_known_answer() {
        let conversation: rim_core::conversation::model::ConversationId =
            uuid::Uuid::parse_str("0199d1a0-0000-7000-8000-000000000001")
                .expect("uuid")
                .into();
        let id = mls_group_id(conversation, 1);
        let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "72696d310199d1a000007000800000000000000100000001");
        assert_eq!(parse_mls_group_id(&id), Some((conversation, 1)));
        assert_eq!(parse_mls_group_id(&id[..23]), None);
        assert_eq!(parse_mls_group_id(b"rim2aaaaaaaaaaaaaaaaaaaa"), None);
    }
}
