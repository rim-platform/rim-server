use async_graphql::*;
use rim_services::Actor;
use rim_services::operator::InstanceOperatorService;
use rim_services::user::command::GetMeQuery;
use rim_services::user::service::UserService;

use crate::auth_context::require_actor;
use crate::types::User;

#[derive(Default)]
pub(crate) struct ProfileQueries;

#[Object]
impl ProfileQueries {
    async fn me(&self, ctx: &Context<'_>) -> Result<User> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn UserService>>()?;

        let dto = svc
            .get_me(GetMeQuery {
                actor: Actor::new(actor),
            })
            .await
            .map_err(crate::errors::map_service_error)?;
        let mut me = User::from(dto);
        if ctx.look_ahead().field("probationEndsAt").exists() {
            me.probation_ends_at = svc
                .probation_ends_at(GetMeQuery {
                    actor: Actor::new(actor),
                })
                .await
                .map_err(crate::errors::map_service_error)?;
        }

        if ctx.look_ahead().field("instanceOperator").exists() {
            me.instance_operator = ctx
                .data::<std::sync::Arc<dyn InstanceOperatorService>>()?
                .is_operator(Actor::new(actor))
                .await
                .map_err(crate::errors::map_service_error)?;
        }

        Ok(me)
    }
}
