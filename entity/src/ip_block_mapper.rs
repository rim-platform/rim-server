use crate::ip_block;
use rim_core::ip_block::model::IpBlock;

impl From<ip_block::Model> for IpBlock {
    fn from(m: ip_block::Model) -> Self {
        Self {
            id: m.id.into(),
            cidr: m.cidr,
            reason: m.reason,
            expires_at: m.expires_at.map(Into::into),
            created_at: m.created_at.into(),
        }
    }
}
