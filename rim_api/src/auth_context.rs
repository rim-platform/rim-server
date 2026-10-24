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
    /// The device the session is bound to; `None` for a password session.
    device_key_id: Option<rim_core::key_identity::model::DeviceKeyId>,
    /// The session may only set a password, sign out and read who it is.
    must_set_password: bool,
}

impl AuthActorCtx {
    pub fn new(
        actor: Option<rim_core::user::model::UserId>,
        session_id: Option<rim_core::auth::model::SessionId>,
    ) -> Self {
        Self {
            actor,
            session_id,
            device_key_id: None,
            must_set_password: false,
        }
    }

    /// The context of a session that may only set a password when `must_set_password`.
    pub fn with_must_set_password(mut self, must_set_password: bool) -> Self {
        self.must_set_password = must_set_password;
        self
    }

    pub fn must_set_password(&self) -> bool {
        self.must_set_password
    }

    /// The context of a session bound to `device_key_id`.
    pub fn with_device_key_id(
        mut self,
        device_key_id: Option<rim_core::key_identity::model::DeviceKeyId>,
    ) -> Self {
        self.device_key_id = device_key_id;
        self
    }

    pub fn device_key_id(&self) -> Option<rim_core::key_identity::model::DeviceKeyId> {
        self.device_key_id
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

fn password_first_error() -> Error {
    Error::new("set a password first").extend_with(|_, e| {
        e.set("code", Value::from("FORBIDDEN"));
    })
}

/// The signed-in user, or `None` for an anonymous request. A session that must set a password
/// first counts as anonymous.
pub(crate) fn optional_actor(ctx: &Context<'_>) -> Option<rim_core::user::model::UserId> {
    ctx.data::<AuthActorCtx>()
        .ok()
        .filter(|auth| !auth.must_set_password())
        .and_then(AuthActorCtx::actor)
}

/// The signed-in user.
///
/// # Errors
/// `UNAUTHENTICATED` without a session; `FORBIDDEN` for a session that must set a password
/// first.
pub fn require_actor(ctx: &Context<'_>) -> Result<rim_core::user::model::UserId, Error> {
    let actor = require_actor_before_password(ctx)?;
    if ctx
        .data::<AuthActorCtx>()
        .is_ok_and(AuthActorCtx::must_set_password)
    {
        return Err(password_first_error());
    }
    Ok(actor)
}

/// The signed-in user, including a session that must set a password first: for setting it,
/// signing out and `me`.
///
/// # Errors
/// `UNAUTHENTICATED` without a session.
pub fn require_actor_before_password(
    ctx: &Context<'_>,
) -> Result<rim_core::user::model::UserId, Error> {
    ctx.data::<AuthActorCtx>()
        .map_err(|_| {
            Error::new("internal error: missing auth context").extend_with(|_, e| {
                e.set("code", Value::from("INTERNAL"));
            })
        })?
        .actor()
        .ok_or_else(unauthenticated_error)
}

/// The signed-in user and the device their session is bound to. Services refuse MLS calls
/// without the device (`FORBIDDEN`).
pub(crate) fn require_mls_caller(
    ctx: &Context<'_>,
) -> Result<rim_services::e2ee::command::MlsCaller, Error> {
    let actor = require_actor(ctx)?;
    let device_key_id = ctx
        .data::<AuthActorCtx>()
        .ok()
        .and_then(AuthActorCtx::device_key_id);
    Ok(rim_services::e2ee::command::MlsCaller {
        actor: rim_services::Actor::new(actor),
        device_key_id,
    })
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
