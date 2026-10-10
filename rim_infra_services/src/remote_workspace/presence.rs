//! Which local users have a relay stream open, so the relay reports them online to the hosts of
//! their workspaces when the first opens and offline once the last has been closed for the
//! debounce period.

use std::collections::HashMap;
use std::sync::Mutex;

use rim_core::user::model::UserId;

/// How long the last stream must stay closed before the user is reported offline.
pub(crate) const OFFLINE_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Default)]
struct Streams {
    open: u32,
    /// Online was reported and offline was not.
    reported_online: bool,
    /// Bumped on every change, so a delayed offline check can tell nothing happened since.
    generation: u64,
}

/// Open relay streams per local user.
#[derive(Debug, Default)]
pub(crate) struct PresenceTracker {
    users: Mutex<HashMap<UserId, Streams>>,
}

impl PresenceTracker {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<UserId, Streams>> {
        match self.users.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// A stream opened. Returns whether to report the user online now.
    pub(crate) fn opened(&self, user_id: UserId) -> bool {
        let mut users = self.lock();
        let streams = users.entry(user_id).or_default();
        streams.open += 1;
        streams.generation += 1;
        if streams.reported_online {
            return false;
        }
        streams.reported_online = true;
        true
    }

    /// A stream closed. Returns the generation to settle after the debounce when it was the
    /// last one.
    pub(crate) fn closed(&self, user_id: UserId) -> Option<u64> {
        let mut users = self.lock();
        let streams = users.get_mut(&user_id)?;
        streams.open = streams.open.saturating_sub(1);
        streams.generation += 1;
        (streams.open == 0).then_some(streams.generation)
    }

    /// The debounce after the last close of `generation` passed. Returns whether to report the
    /// user offline: nothing opened since and online was reported.
    pub(crate) fn settle(&self, user_id: UserId, generation: u64) -> bool {
        let mut users = self.lock();
        let Some(streams) = users.get(&user_id) else {
            return false;
        };
        if streams.generation != generation || streams.open > 0 {
            return false;
        }
        let report = streams.reported_online;
        users.remove(&user_id);
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_open_reports_online_and_the_last_close_offline_after_settling() {
        let tracker = PresenceTracker::default();
        let user = UserId::new();
        assert!(tracker.opened(user));
        assert!(!tracker.opened(user), "a second stream reports nothing");
        assert_eq!(tracker.closed(user), None, "one stream is still open");
        let generation = tracker.closed(user).expect("the last stream closed");
        assert!(tracker.settle(user, generation));
        assert!(!tracker.settle(user, generation), "settled once");
    }

    #[test]
    fn a_quick_reconnect_reports_nothing() {
        let tracker = PresenceTracker::default();
        let user = UserId::new();
        assert!(tracker.opened(user));
        let generation = tracker.closed(user).expect("closed");
        assert!(!tracker.opened(user), "still reported online");
        assert!(
            !tracker.settle(user, generation),
            "reopened within the debounce"
        );
        let later = tracker.closed(user).expect("closed again");
        assert!(tracker.settle(user, later));
    }

    #[test]
    fn a_close_without_an_open_is_ignored() {
        let tracker = PresenceTracker::default();
        assert_eq!(tracker.closed(UserId::new()), None);
    }
}
