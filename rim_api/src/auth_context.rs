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

/// Fail unless `actor` is a member of `server_id`.
pub(crate) async fn require_server_member(
    ctx: &Context<'_>,
    actor: rim_core::user::model::UserId,
    server_id: rim_core::server::model::ServerId,
) -> Result<(), Error> {
    let member_svc = ctx
        .data::<std::sync::Arc<dyn rim_services::server_member::service::ServerMemberService>>()?;
    member_svc
        .get_member(rim_services::server_member::command::GetServerMemberQuery {
            actor: rim_services::Actor::new(actor),
            server_id,
            user_id: actor,
        })
        .await
        .map_err(crate::errors::map_service_error)?;
    Ok(())
}

/// The caller's IP for rate limiting, or `"unknown"` if the handler did not inject one.
pub(crate) fn remote_ip(ctx: &Context<'_>) -> String {
    ctx.data::<RemoteIp>()
        .map(|r| r.0.clone())
        .unwrap_or_else(|_| "unknown".to_string())
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
