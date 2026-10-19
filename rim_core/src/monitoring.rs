use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::repository::error::RepoResult;

/// Current local accounts and distinct authenticated accounts in rolling windows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsageStats {
    pub registered_accounts: i64,
    pub active_24h: i64,
    pub active_7d: i64,
}

/// Pending recipient rows and their outstanding device deliveries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeliveryStats {
    pub items: i64,
    pub bytes: i64,
    pub device_items: i64,
    pub users: i64,
    pub devices: i64,
    pub oldest_age_seconds: f64,
}

/// Aggregate storage reads without account or message identifiers in the result.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MonitoringRepository: Send + Sync {
    /// Current local accounts and their rolling authenticated activity.
    async fn usage_stats(&self) -> RepoResult<UsageStats>;

    /// Unexpired, unheld items awaiting at least one non-revoked eligible device.
    async fn delivery_stats(&self) -> RepoResult<DeliveryStats>;
}
