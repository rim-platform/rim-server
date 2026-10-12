use chrono::{DateTime, Utc};
use rim_core::ip_block::model::IpRange;

/// Block an address or range: `cidr` is an address or a CIDR range, v4 or v6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockIpCmd {
    pub cidr: String,
    pub reason: String,
    /// Days until the block lifts, 1 to 365; `None` blocks until it is removed.
    pub expires_in_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpBlockDto {
    pub range: IpRange,
    pub reason: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpBlockOutcome {
    pub block: IpBlockDto,
    /// `false` when the range was blocked already and its reason and expiry were replaced.
    pub newly_blocked: bool,
}
