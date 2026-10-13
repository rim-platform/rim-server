use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::ip_block::dto::{BlockIpCmd, IpBlockDto, IpBlockOutcome};
use crate::operator::OperatorCaller;

/// Days a block may last at most.
pub const MAX_BLOCK_DAYS: u32 = 365;

/// The operator's IP deny list: addresses and ranges refused on every route but the liveness
/// probe. The methods that take who runs them refuse a user who is not an instance operator
/// with `Forbidden`.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait IpBlockService: Send + Sync {
    /// Block an address or range for a reason, or replace the reason and expiry of its block.
    async fn block(&self, by: OperatorCaller, cmd: BlockIpCmd) -> ServiceResult<IpBlockOutcome>;

    /// Lift the block on an address or range, given as it was blocked or in any form that
    /// names the same range; whether there was one.
    async fn unblock(&self, by: OperatorCaller, cidr: String) -> ServiceResult<bool>;

    /// Every block, expired or not, oldest first.
    async fn list(&self, by: OperatorCaller) -> ServiceResult<Vec<IpBlockDto>>;

    /// The blocks in force now.
    async fn active(&self) -> ServiceResult<Vec<IpBlockDto>>;

    /// Delete the expired blocks; how many.
    async fn purge_expired(&self) -> ServiceResult<u64>;
}
