mod in_memory_scheduler;
mod policy_service;

pub use in_memory_scheduler::InMemoryEscalationScheduler;
pub use policy_service::{DefaultEscalationPolicyService, EscalationPolicyRepos};

use rim_core::escalation::repository::EscalationRunRepository;
use rim_services::escalation::scheduler::EscalationScheduler;

/// Boot recovery (§6): re-arms every `status = 'active'` escalation run from before a restart.
/// A `next_fire_at` already in the past is armed exactly the same as a future one — the driver
/// task's `sleep_until`-style loop resolves an already-past deadline immediately, which is the
/// correct "missed page during downtime pages ASAP" behavior. A run with a missing
/// `next_fire_at` (should never happen for an `Active` row — see `docs/oncall.md`) is skipped
/// with a `warn` log rather than panicking.
///
/// Returns the count of runs successfully armed (for the boot audit line). Extracted as a free
/// function (rather than inlined in `rim_server::app::build_app_runtime`) so it is independently
/// db-testable without spinning up the whole app runtime.
pub async fn recover_active_escalation_runs(
    run_repo: &dyn EscalationRunRepository,
    scheduler: &dyn EscalationScheduler,
) -> rim_core::repository::error::RepoResult<usize> {
    let runs = run_repo.list_active().await?;
    let mut armed = 0usize;
    for run in runs {
        match run.next_fire_at {
            Some(fire_at) => {
                scheduler.arm(run.id, fire_at);
                armed += 1;
            }
            None => {
                tracing::warn!(
                    run_id = %run.id,
                    "boot recovery: active escalation run has no next_fire_at, skipping"
                );
            }
        }
    }
    Ok(armed)
}
