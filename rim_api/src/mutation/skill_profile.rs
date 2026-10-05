use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;

#[derive(Default)]
pub(crate) struct SkillProfileMutations;

#[Object]
impl SkillProfileMutations {
    // ── Skill Profile ──────────────────────────────────────────────────

    async fn record_feature_usage(
        &self,
        ctx: &Context<'_>,
        feature_id: String,
    ) -> Result<crate::types::SkillProfileGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::skill_profile::service::SkillProfileService>>(
            )?;

        let profile = svc
            .record_feature_usage(rim_services::skill_profile::dto::RecordFeatureUsageCmd {
                actor: Actor::new(actor),
                feature_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(profile.into())
    }

    async fn override_skill_tier(
        &self,
        ctx: &Context<'_>,
        tier: crate::types::SkillTier,
    ) -> Result<crate::types::SkillProfileGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::skill_profile::service::SkillProfileService>>(
            )?;

        let profile = svc
            .override_skill_tier(rim_services::skill_profile::dto::OverrideSkillTierCmd {
                actor: Actor::new(actor),
                tier: tier.into(),
            })
            .await
            .map_err(map_service_error)?;

        Ok(profile.into())
    }
}
