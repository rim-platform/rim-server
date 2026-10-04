use std::time::Instant;

use dashmap::DashMap;
use rim_core::conversation::model::ConversationId;
use rim_core::user::model::UserId;
use rim_services::typing::TypingService;

/// Minimum interval between accepted typing broadcasts for the same user
/// in the same conversation (3 seconds).
const TYPING_THROTTLE_SECS: u64 = 3;

/// Entries older than this are considered stale and will be removed by `cleanup_stale`.
const STALE_ENTRY_SECS: u64 = 30;

/// In-memory tracker that throttles typing indicator broadcasts.
pub struct TypingTracker {
    last_typing: DashMap<(UserId, ConversationId), Instant>,
}

impl Default for TypingTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl TypingTracker {
    pub fn new() -> Self {
        Self {
            last_typing: DashMap::new(),
        }
    }

    /// Returns `true` if enough time has passed since the last broadcast
    /// for this user+conversation pair and the event should be emitted.
    pub fn should_broadcast(&self, user_id: UserId, conversation_id: ConversationId) -> bool {
        let now = Instant::now();
        let key = (user_id, conversation_id);

        if let Some(last) = self.last_typing.get(&key)
            && now.duration_since(*last).as_secs() < TYPING_THROTTLE_SECS
        {
            return false;
        }

        self.last_typing.insert(key, now);
        true
    }

    /// Remove entries older than 30 seconds to prevent unbounded growth.
    pub fn cleanup_stale(&self) {
        let now = Instant::now();
        self.last_typing
            .retain(|_key, last| now.duration_since(*last).as_secs() < STALE_ENTRY_SECS);
    }
}

impl TypingService for TypingTracker {
    fn should_broadcast(&self, user_id: UserId, conversation_id: ConversationId) -> bool {
        self.should_broadcast(user_id, conversation_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_typing_is_always_broadcast() {
        let tracker = TypingTracker::new();
        let uid = UserId::new();
        let cid = ConversationId::new();
        assert!(tracker.should_broadcast(uid, cid));
    }

    #[test]
    fn rapid_typing_is_throttled() {
        let tracker = TypingTracker::new();
        let uid = UserId::new();
        let cid = ConversationId::new();
        assert!(tracker.should_broadcast(uid, cid));
        assert!(!tracker.should_broadcast(uid, cid));
    }

    #[test]
    fn different_conversations_are_independent() {
        let tracker = TypingTracker::new();
        let uid = UserId::new();
        let cid1 = ConversationId::new();
        let cid2 = ConversationId::new();
        assert!(tracker.should_broadcast(uid, cid1));
        assert!(tracker.should_broadcast(uid, cid2));
    }

    #[test]
    fn different_users_are_independent() {
        let tracker = TypingTracker::new();
        let uid1 = UserId::new();
        let uid2 = UserId::new();
        let cid = ConversationId::new();
        assert!(tracker.should_broadcast(uid1, cid));
        assert!(tracker.should_broadcast(uid2, cid));
    }
}
