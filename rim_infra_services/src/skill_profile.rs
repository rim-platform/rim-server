use std::sync::Arc;

use async_trait::async_trait;

use rim_core::skill_profile::model::SkillTier;
use rim_core::skill_profile::repository::SkillProfileRepository;
use rim_services::skill_profile::dto::{
    OverrideSkillTierCmd, RecordFeatureUsageCmd, SkillProfileDto,
};
use rim_services::skill_profile::service::SkillProfileService;
use rim_services::{Actor, ServiceResult};

/// Number of distinct features required to auto-promote to intermediate.
const INTERMEDIATE_THRESHOLD: usize = 5;

/// Number of distinct features required to auto-promote to advanced.
const ADVANCED_THRESHOLD: usize = 15;

pub struct DefaultSkillProfileService {
    skill_profile_repo: Arc<dyn SkillProfileRepository>,
}

impl DefaultSkillProfileService {
    pub fn new(skill_profile_repo: Arc<dyn SkillProfileRepository>) -> Self {
        Self { skill_profile_repo }
    }
}

fn auto_tier(feature_count: usize) -> SkillTier {
    if feature_count >= ADVANCED_THRESHOLD {
        SkillTier::Advanced
    } else if feature_count >= INTERMEDIATE_THRESHOLD {
        SkillTier::Intermediate
    } else {
        SkillTier::Beginner
    }
}

#[async_trait]
impl SkillProfileService for DefaultSkillProfileService {
    async fn get_skill_profile(&self, actor: Actor) -> ServiceResult<SkillProfileDto> {
        let profile = self.skill_profile_repo.get_or_create(actor.user_id).await?;

        Ok(SkillProfileDto::from(profile))
    }

    async fn record_feature_usage(
        &self,
        cmd: RecordFeatureUsageCmd,
    ) -> ServiceResult<SkillProfileDto> {
        let mut profile = self
            .skill_profile_repo
            .get_or_create(cmd.actor.user_id)
            .await?;

        // Idempotent: only record first use
        profile
            .features_used
            .entry(cmd.feature_id)
            .or_insert_with(chrono::Utc::now);

        // Auto-promote based on feature count
        let new_tier = auto_tier(profile.features_used.len());

        let updated = self
            .skill_profile_repo
            .update(cmd.actor.user_id, &profile.features_used, new_tier)
            .await?;

        Ok(SkillProfileDto::from(updated))
    }

    async fn override_skill_tier(
        &self,
        cmd: OverrideSkillTierCmd,
    ) -> ServiceResult<SkillProfileDto> {
        let profile = self
            .skill_profile_repo
            .get_or_create(cmd.actor.user_id)
            .await?;

        let updated = self
            .skill_profile_repo
            .update(cmd.actor.user_id, &profile.features_used, cmd.tier)
            .await?;

        Ok(SkillProfileDto::from(updated))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_tier_beginner_below_threshold() {
        assert_eq!(auto_tier(0), SkillTier::Beginner);
        assert_eq!(auto_tier(4), SkillTier::Beginner);
    }

    #[test]
    fn auto_tier_intermediate_at_threshold() {
        assert_eq!(auto_tier(5), SkillTier::Intermediate);
        assert_eq!(auto_tier(14), SkillTier::Intermediate);
    }

    #[test]
    fn auto_tier_advanced_at_threshold() {
        assert_eq!(auto_tier(15), SkillTier::Advanced);
        assert_eq!(auto_tier(100), SkillTier::Advanced);
    }
}
