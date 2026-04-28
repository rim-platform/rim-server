use async_graphql::{Context, Error, ErrorExtensions, Value};

/// Per-request remote IP address, injected by the Actix handler.
///
/// Resolvers that need the caller's IP (e.g. `previewInviteToken` rate-limiter)
/// read this from context. Falls back to `"unknown"` if the HTTP handler did
/// not inject it — that case is a configuration/wiring mistake, not an attack
/// surface, because rate-limit keys on `"unknown"` still work (they just bucket
/// all unknown IPs together).
#[derive(Debug, Clone, Default)]
pub struct RemoteIp(pub String);

impl RemoteIp {
    /// Create a new `RemoteIp` from the given string.
    pub fn new(ip: impl Into<String>) -> Self {
        Self(ip.into())
    }

    /// Return the contained IP string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

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

fn unauthenticated_error() -> Error {
    Error::new("unauthenticated").extend_with(|_, e| {
        e.set("code", Value::from("UNAUTHENTICATED"));
    })
}

pub fn require_actor(ctx: &Context<'_>) -> Result<rim_core::user::model::UserId, Error> {
    ctx.data::<AuthActorCtx>()
        .map_err(|_| {
            Error::new("internal error: missing auth context").extend_with(|_, e| {
                e.set("code", Value::from("INTERNAL"));
            })
        })?
        .actor()
        .ok_or_else(unauthenticated_error)
}

pub fn require_session_id(ctx: &Context<'_>) -> Result<rim_core::auth::model::SessionId, Error> {
    ctx.data::<AuthActorCtx>()
        .map_err(|_| {
            Error::new("internal error: missing auth context").extend_with(|_, e| {
                e.set("code", Value::from("INTERNAL"));
            })
        })?
        .session_id()
        .ok_or_else(unauthenticated_error)
}
