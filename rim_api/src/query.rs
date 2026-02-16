use async_graphql::*;

#[derive(Default)]
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

    // Cookie/CSRF signaling is handled via GraphQL response extensions.
}

pub struct Query;

#[Object]
impl Query {
    async fn hello_world(&self, _ctx: &Context<'_>) -> String {
        "Hello world!".to_string()
    }
}
