use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ip_block::model::{IpBlock, NewIpBlock};
use crate::repository::error::RepoResult;

/// The addresses and ranges this server refuses, keyed by their canonical CIDR text.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait IpBlockRepository: Send + Sync {
    /// Insert the block, or replace the reason and expiry of the block on the same range.
    /// Returns the stored block and whether it is new.
    async fn upsert(&self, block: NewIpBlock) -> RepoResult<(IpBlock, bool)>;

    /// Remove the block on `cidr`; whether there was one.
    async fn delete(&self, cidr: &str) -> RepoResult<bool>;

    /// Every block, expired or not, oldest first.
    async fn list(&self) -> RepoResult<Vec<IpBlock>>;

    /// The blocks without an expiry or expiring after `now`, oldest first.
    async fn list_active(&self, now: DateTime<Utc>) -> RepoResult<Vec<IpBlock>>;

    /// Delete the blocks that expired at or before `now`; how many.
    async fn delete_expired(&self, now: DateTime<Utc>) -> RepoResult<u64>;
}
