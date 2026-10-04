use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::Actor;
use crate::ServiceResult;
use crate::skill_profile::dto::{OverrideSkillTierCmd, RecordFeatureUsageCmd, SkillProfileDto};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait SkillProfileService: Send + Sync {
    async fn get_skill_profile(&self, actor: Actor) -> ServiceResult<SkillProfileDto>;

    async fn record_feature_usage(
        &self,
        cmd: RecordFeatureUsageCmd,
    ) -> ServiceResult<SkillProfileDto>;

    async fn override_skill_tier(
        &self,
        cmd: OverrideSkillTierCmd,
    ) -> ServiceResult<SkillProfileDto>;
}
