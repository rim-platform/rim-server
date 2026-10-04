use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::Notify;

use rim_core::escalation::model::EscalationRunId;
use rim_services::escalation::scheduler::EscalationScheduler;

/// Single-node in-memory timer wheel (§6). Holds only `(run_id, fire_at)` pairs — the DB
/// `escalation_run` row is the source of truth; this struct is purely the timing mechanism the
/// driver task in `rim_server::app` polls.
///
/// `arm`/`cancel` are cheap, synchronous map operations that also wake any driver task blocked
/// in [`Self::notified`], so a newly-armed run with an earlier deadline than whatever the driver
/// was sleeping on gets picked up immediately rather than waiting out the old deadline.
///
/// **Multi-node caveat** (mirrors `fable-notes.md` §7): this state lives in one process's
/// memory. A second node would maintain its own independent set of armed timers and could
/// double-page on the same run, or lose armed timers entirely on that node's restart (boot
/// recovery, run by whichever node starts, covers the single-node restart case — see
/// `docs/oncall.md`). This is an availability/UX regression under naive multi-node, not a
/// security one; the trait boundary is the swap point for a `SELECT ... FOR UPDATE SKIP LOCKED`
/// poller or a Redis-backed scheduler later.
pub struct InMemoryEscalationScheduler {
    deadlines: DashMap<EscalationRunId, DateTime<Utc>>,
    notify: Notify,
}

impl InMemoryEscalationScheduler {
    pub fn new() -> Self {
        Self {
            deadlines: DashMap::new(),
            notify: Notify::new(),
        }
    }

    /// The earliest deadline across every currently-armed run, if any. The driver task sleeps
    /// until this instant (or wakes early via [`Self::notified`]).
    pub fn next_deadline(&self) -> Option<DateTime<Utc>> {
        self.deadlines.iter().map(|entry| *entry.value()).min()
    }

    /// Removes and returns every run whose deadline is `<= now`. Called by the driver task after
    /// waking; the caller is responsible for invoking the executor for each returned id.
    pub fn take_due(&self, now: DateTime<Utc>) -> Vec<EscalationRunId> {
        let due: Vec<EscalationRunId> = self
            .deadlines
            .iter()
            .filter(|entry| *entry.value() <= now)
            .map(|entry| *entry.key())
            .collect();
        for run_id in &due {
            self.deadlines.remove(run_id);
        }
        due
    }

    /// Resolves when `arm`/`cancel` changes the armed set. The driver task races this against
    /// its deadline sleep so a newly-armed earlier deadline (or a cancellation) is noticed
    /// immediately rather than after the stale sleep elapses.
    pub async fn notified(&self) {
        self.notify.notified().await;
    }
}

impl Default for InMemoryEscalationScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl EscalationScheduler for InMemoryEscalationScheduler {
    fn arm(&self, run_id: EscalationRunId, fire_at: DateTime<Utc>) {
        self.deadlines.insert(run_id, fire_at);
        self.notify.notify_waiters();
    }

    fn cancel(&self, run_id: EscalationRunId) {
        self.deadlines.remove(&run_id);
        self.notify.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn new_scheduler_has_no_next_deadline() {
        let scheduler = InMemoryEscalationScheduler::new();
        assert_eq!(scheduler.next_deadline(), None);
    }

    #[test]
    fn arm_sets_next_deadline() {
        let scheduler = InMemoryEscalationScheduler::new();
        let run_id = EscalationRunId::new();
        let fire_at = Utc::now() + Duration::seconds(30);
        scheduler.arm(run_id, fire_at);
        assert_eq!(scheduler.next_deadline(), Some(fire_at));
    }

    #[test]
    fn next_deadline_is_the_earliest_of_multiple_armed_runs() {
        let scheduler = InMemoryEscalationScheduler::new();
        let now = Utc::now();
        let earlier = now + Duration::seconds(10);
        let later = now + Duration::seconds(60);
        scheduler.arm(EscalationRunId::new(), later);
        scheduler.arm(EscalationRunId::new(), earlier);
        assert_eq!(scheduler.next_deadline(), Some(earlier));
    }

    #[test]
    fn cancel_removes_the_run_from_the_armed_set() {
        let scheduler = InMemoryEscalationScheduler::new();
        let run_id = EscalationRunId::new();
        scheduler.arm(run_id, Utc::now() + Duration::seconds(30));
        scheduler.cancel(run_id);
        assert_eq!(scheduler.next_deadline(), None);
    }

    #[test]
    fn cancel_of_unarmed_run_is_a_no_op() {
        let scheduler = InMemoryEscalationScheduler::new();
        // Must not panic.
        scheduler.cancel(EscalationRunId::new());
        assert_eq!(scheduler.next_deadline(), None);
    }

    #[test]
    fn re_arming_a_run_updates_its_deadline_idempotently() {
        let scheduler = InMemoryEscalationScheduler::new();
        let run_id = EscalationRunId::new();
        let first = Utc::now() + Duration::seconds(30);
        let second = Utc::now() + Duration::seconds(5);
        scheduler.arm(run_id, first);
        scheduler.arm(run_id, second);
        assert_eq!(scheduler.next_deadline(), Some(second));
    }

    #[test]
    fn take_due_returns_only_runs_at_or_before_now_and_removes_them() {
        let scheduler = InMemoryEscalationScheduler::new();
        let now = Utc::now();
        let due_run = EscalationRunId::new();
        let future_run = EscalationRunId::new();
        scheduler.arm(due_run, now - Duration::seconds(1));
        scheduler.arm(future_run, now + Duration::seconds(60));

        let due = scheduler.take_due(now);
        assert_eq!(due, vec![due_run]);
        // The due run was removed; the future one remains armed.
        assert_eq!(scheduler.next_deadline(), Some(now + Duration::seconds(60)));
    }

    #[tokio::test]
    async fn arm_wakes_a_task_blocked_on_notified() {
        use std::sync::Arc;

        let scheduler = Arc::new(InMemoryEscalationScheduler::new());
        let waiter = Arc::clone(&scheduler);
        let handle = tokio::spawn(async move {
            waiter.notified().await;
        });

        // Give the spawned task a chance to reach `.notified().await` before we arm.
        tokio::task::yield_now().await;
        scheduler.arm(EscalationRunId::new(), Utc::now());

        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("notified() must resolve after arm()")
            .expect("spawned task must not panic");
    }
}
