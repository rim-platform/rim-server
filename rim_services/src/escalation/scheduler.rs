use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::escalation::model::EscalationRunId;

/// The single-node in-memory timer wheel behind a trait (§6). The DB `escalation_run` row is
/// the source of truth; this trait is only the *timing mechanism* — it does not read or write
/// any run state itself. `arm`/`cancel` are synchronous: the in-memory implementation just
/// updates a map and wakes the driver task, with no I/O on the call path.
///
/// **At-least-once paging guarantee:** a fire that races a cancel (ack/resolve) is handled by
/// the *executor* re-checking run status on every fire, not by this trait — see
/// `docs/oncall.md` "Escalation wiring" for the full boot-recovery/race discussion. This is the
/// swap point for a future DB-poller or Redis-backed scheduler (multi-node caveat, §6).
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait EscalationScheduler: Send + Sync {
    /// Arm (or re-arm) a run to fire at `fire_at`. Idempotent per `run_id` — arming an
    /// already-armed run simply updates its deadline.
    fn arm(&self, run_id: EscalationRunId, fire_at: DateTime<Utc>);

    /// Cancel a pending timer (ack/resolve/mitigate). No-op if `run_id` isn't armed.
    fn cancel(&self, run_id: EscalationRunId);
}
