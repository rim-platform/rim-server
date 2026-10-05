use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;

#[derive(Default)]
pub(crate) struct SkillProfileQueries;

#[Object]
impl SkillProfileQueries {
    // ── Skill Profile ──────────────────────────────────────────────────

    async fn my_skill_profile(&self, ctx: &Context<'_>) -> Result<crate::types::SkillProfileGql> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::skill_profile::service::SkillProfileService>>(
            )?;

        let profile = svc
            .get_skill_profile(Actor::new(actor))
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(profile.into())
    }
}
