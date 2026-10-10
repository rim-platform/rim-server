//! Background purges the composition root runs on a timer: stale attachment uploads are marked
//! failed daily, expired workspace state is deleted hourly. Each runs first a few minutes after
//! boot and stops when the shutdown signal changes.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use rim_core::workspace_state::repository::WorkspaceStateRepository;
use rim_services::message_attachment::service::MessageAttachmentService;

use crate::message_attachment::DefaultMessageAttachmentService;

/// Delay before the first run of each purge, so they stay out of the way of a starting server.
const FIRST_RUN_DELAY: Duration = Duration::from_secs(5 * 60);
/// Interval of the stale-attachment cleanup.
pub const ATTACHMENT_CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Interval of the workspace-state expiry purge.
pub const WORKSPACE_STATE_PURGE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Run `job` after [`FIRST_RUN_DELAY`] and then every `interval` until `shutdown` changes. A run
/// that falls behind delays the next one rather than bunching them.
async fn every<F, Fut>(interval: Duration, mut shutdown: watch::Receiver<bool>, mut job: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    let mut tick =
        tokio::time::interval_at(tokio::time::Instant::now() + FIRST_RUN_DELAY, interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => job().await,
            _ = shutdown.changed() => break,
        }
    }
}

/// Mark the uploads still pending or uploaded past the service's staleness threshold as failed.
/// Returns how many, `None` when the cleanup failed (logged).
pub async fn clean_stale_attachments(service: &dyn MessageAttachmentService) -> Option<u64> {
    match service
        .cleanup_stale_attachments(DefaultMessageAttachmentService::STALE_THRESHOLD_MINUTES)
        .await
    {
        Ok(count) => {
            crate::metrics::attachments_cleaned(count);
            tracing::info!(count, "marked stale attachment uploads failed");
            Some(count)
        }
        Err(e) => {
            tracing::error!(error = %e, "stale attachment cleanup failed");
            None
        }
    }
}

/// Delete the workspace state entries past their expiry. Returns how many, `None` when the
/// purge failed (logged).
pub async fn purge_expired_workspace_state(repo: &dyn WorkspaceStateRepository) -> Option<u64> {
    match repo.delete_expired().await {
        Ok(count) => {
            crate::metrics::workspace_state_purged(count);
            tracing::info!(count, "purged expired workspace state");
            Some(count)
        }
        Err(e) => {
            tracing::error!(error = %e, "workspace state expiry purge failed");
            None
        }
    }
}

/// Run [`clean_stale_attachments`] daily until `shutdown` changes.
pub async fn run_attachment_cleanup(
    service: Arc<dyn MessageAttachmentService>,
    shutdown: watch::Receiver<bool>,
) {
    every(ATTACHMENT_CLEANUP_INTERVAL, shutdown, || {
        let service = Arc::clone(&service);
        async move {
            clean_stale_attachments(service.as_ref()).await;
        }
    })
    .await;
}

/// Run [`purge_expired_workspace_state`] hourly until `shutdown` changes.
pub async fn run_workspace_state_purge(
    repo: Arc<dyn WorkspaceStateRepository>,
    shutdown: watch::Receiver<bool>,
) {
    every(WORKSPACE_STATE_PURGE_INTERVAL, shutdown, || {
        let repo = Arc::clone(&repo);
        async move {
            purge_expired_workspace_state(repo.as_ref()).await;
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    use rim_core::repository::error::{RepoError, RepoErrorKind};
    use rim_core::workspace_state::repository::MockWorkspaceStateRepository;
    use rim_services::ServiceError;
    use rim_services::message_attachment::service::MockMessageAttachmentService;

    use super::*;
    use crate::metrics::test_support::{block_on, recorded};
    use crate::metrics::{ATTACHMENTS_CLEANED, WORKSPACE_STATE_PURGED};

    #[test]
    fn the_attachment_cleanup_uses_the_services_threshold_and_counts_what_it_marked() {
        let mut service = MockMessageAttachmentService::new();
        service
            .expect_cleanup_stale_attachments()
            .withf(|minutes| *minutes == DefaultMessageAttachmentService::STALE_THRESHOLD_MINUTES)
            .times(1)
            .returning(|_| Box::pin(async { Ok(3) }));
        let (count, got) = recorded(|| block_on(clean_stale_attachments(&service)));
        assert_eq!(count, Some(3));
        assert_eq!(got.counter(ATTACHMENTS_CLEANED, &[]), 3);
    }

    #[test]
    fn a_failed_attachment_cleanup_counts_nothing() {
        let mut service = MockMessageAttachmentService::new();
        service
            .expect_cleanup_stale_attachments()
            .returning(|_| Box::pin(async { Err(ServiceError::NotFound) }));
        let (count, got) = recorded(|| block_on(clean_stale_attachments(&service)));
        assert_eq!(count, None);
        assert_eq!(got.counter(ATTACHMENTS_CLEANED, &[]), 0);
    }

    #[test]
    fn the_workspace_state_purge_counts_what_it_deleted() {
        let mut repo = MockWorkspaceStateRepository::new();
        repo.expect_delete_expired()
            .times(1)
            .returning(|| Box::pin(async { Ok(7) }));
        let (count, got) = recorded(|| block_on(purge_expired_workspace_state(&repo)));
        assert_eq!(count, Some(7));
        assert_eq!(got.counter(WORKSPACE_STATE_PURGED, &[]), 7);
    }

    #[test]
    fn a_failed_workspace_state_purge_counts_nothing() {
        let mut repo = MockWorkspaceStateRepository::new();
        repo.expect_delete_expired()
            .returning(|| Box::pin(async { Err(RepoError::new(RepoErrorKind::Internal)) }));
        let (count, got) = recorded(|| block_on(purge_expired_workspace_state(&repo)));
        assert_eq!(count, None);
        assert_eq!(got.counter(WORKSPACE_STATE_PURGED, &[]), 0);
    }

    #[tokio::test]
    async fn a_job_stops_on_shutdown_before_its_first_run() {
        let (tx, rx) = watch::channel(false);
        let mut repo = MockWorkspaceStateRepository::new();
        repo.expect_delete_expired().never();
        let task = tokio::spawn(run_workspace_state_purge(Arc::new(repo), rx));
        tx.send(true).expect("send");
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("stopped")
            .expect("joined");
    }
}
