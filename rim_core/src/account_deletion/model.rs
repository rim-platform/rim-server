use chrono::{DateTime, Utc};

use crate::conversation::model::ConversationId;
use crate::server::model::ServerId;
use crate::user::model::UserId;

/// The display name every purged account shows.
pub const TOMBSTONE_DISPLAY_NAME: &str = "Deleted account";

/// The prefix of every purged account's username; signups may not use it.
pub const TOMBSTONE_USERNAME_PREFIX: &str = "deleted-";

/// Where an account stands with respect to deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStanding {
    pub user_id: UserId,
    pub username: String,
    /// The account is a person from another server, whose relay is elsewhere.
    pub is_remote: bool,
    /// When the pending deletion runs; `None` while none is pending.
    pub deletion_scheduled_for: Option<DateTime<Utc>>,
    /// When the account was purged; `Some` makes it a tombstone.
    pub deleted_at: Option<DateTime<Utc>>,
}

/// A workspace that blocks its owner's deletion: the account is its only owner and other people
/// are members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedSharedWorkspace {
    pub id: ServerId,
    pub name: String,
}

/// An encrypted conversation the purged account was a member of, with the members it keeps.
/// Their clients remove the account's devices from the conversation's group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftConversation {
    pub workspace_id: ServerId,
    pub conversation_id: ConversationId,
    pub member_user_ids: Vec<UserId>,
}

/// What one purge step did: how many rows of each kind it removed or changed, the stored files
/// to delete once the purge commits, and the workspaces and encrypted conversations the account
/// left.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PurgeStep {
    pub counts: Vec<(&'static str, u64)>,
    pub storage_keys: Vec<String>,
    pub left_workspaces: Vec<ServerId>,
    pub left_conversations: Vec<LeftConversation>,
}

impl PurgeStep {
    /// A step that only counted rows.
    pub fn counted(counts: Vec<(&'static str, u64)>) -> Self {
        Self {
            counts,
            ..Self::default()
        }
    }

    /// Add `other`'s counts, files and departures to this step's.
    pub fn absorb(&mut self, other: PurgeStep) {
        self.counts.extend(other.counts);
        self.storage_keys.extend(other.storage_keys);
        self.left_workspaces.extend(other.left_workspaces);
        self.left_conversations.extend(other.left_conversations);
    }
}

/// The usernames a purged account may take, in order of preference: `deleted-` and the first
/// 8 hex digits of its id, then longer prefixes of the id. Ids made close together share their
/// first digits, so the first free candidate is used.
pub fn tombstone_username_candidates(id: UserId) -> Vec<String> {
    let hex = uuid::Uuid::from(id).simple().to_string();
    [8, 12, 16, 24]
        .into_iter()
        .map(|len| format!("{TOMBSTONE_USERNAME_PREFIX}{}", &hex[..len]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tombstone_usernames_start_with_eight_hex_digits_and_fit_the_username_limit() {
        let id = UserId::from(
            uuid::Uuid::parse_str("0199d1a0-1234-7abc-8def-0123456789ab").expect("uuid"),
        );
        assert_eq!(
            tombstone_username_candidates(id),
            vec![
                "deleted-0199d1a0",
                "deleted-0199d1a01234",
                "deleted-0199d1a012347abc",
                "deleted-0199d1a012347abc8def0123",
            ]
        );
        assert!(
            tombstone_username_candidates(UserId::new())
                .iter()
                .all(|name| name.len() <= 32)
        );
    }

    #[test]
    fn a_step_absorbs_another_steps_counts_and_files() {
        let mut step = PurgeStep::counted(vec![("sessions", 2)]);
        step.absorb(PurgeStep {
            counts: vec![("messages", 3)],
            storage_keys: vec!["uploads/a".to_string()],
            left_workspaces: vec![ServerId::new()],
            left_conversations: vec![],
        });
        assert_eq!(step.counts, vec![("sessions", 2), ("messages", 3)]);
        assert_eq!(step.storage_keys, vec!["uploads/a".to_string()]);
        assert_eq!(step.left_workspaces.len(), 1);
    }
}
