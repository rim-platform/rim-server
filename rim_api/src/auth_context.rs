use async_graphql::{Context, Error};

#[derive(Default, Clone)]
pub struct AuthActorCtx {
    actor: Option<rim_core::user::model::UserId>,
    session_id: Option<rim_core::auth::model::SessionId>,
}

impl AuthActorCtx {
    pub fn new(
        actor: Option<rim_core::user::model::UserId>,
        session_id: Option<rim_core::auth::model::SessionId>,
    ) -> Self {
        Self { actor, session_id }
    }

    pub fn actor(&self) -> Option<rim_core::user::model::UserId> {
        self.actor
    }

    pub fn session_id(&self) -> Option<rim_core::auth::model::SessionId> {
        self.session_id
    }
}

pub fn require_actor(ctx: &Context<'_>) -> Result<rim_core::user::model::UserId, Error> {
    ctx.data_unchecked::<AuthActorCtx>()
        .actor()
        .ok_or_else(|| Error::new("unauthenticated"))
}

pub fn require_session_id(ctx: &Context<'_>) -> Result<rim_core::auth::model::SessionId, Error> {
    ctx.data_unchecked::<AuthActorCtx>()
        .session_id()
        .ok_or_else(|| Error::new("unauthenticated"))
}
