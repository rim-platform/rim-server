//! What resolvers need from this server's relay and host roles: the services, the roles this
//! server runs, and the account mutations a person from another server may not run here.

use std::sync::Arc;

use async_graphql::{Context, Error, ErrorExtensions, Value};
use rim_core::user::model::UserId;
use rim_services::host::service::HostService;
use rim_services::relay::dto::RelayInfoDto;
use rim_services::relay::service::RelayService;
use rim_services::remote_workspace::service::{WorkspaceHostService, WorkspaceRelayService};

use crate::errors::map_service_error;
use crate::operation::{Classification, parse_root_fields};

/// The relay and host services and this server's roles, injected into the schema.
#[derive(Clone)]
pub struct FederationContext {
    pub relay: Arc<dyn RelayService>,
    pub host: Arc<dyn HostService>,
    /// Workspaces here with members whose relay is elsewhere.
    pub workspace_host: Arc<dyn WorkspaceHostService>,
    /// This relay's users' workspaces hosted elsewhere.
    pub workspace_relay: Arc<dyn WorkspaceRelayService>,
    pub info: RelayInfoDto,
    /// This server runs the relay role.
    pub relay_role: bool,
    /// This server runs the host role.
    pub host_role: bool,
}

/// Marks a GraphQL request run for a person from another server, through a forwarded request.
#[derive(Debug, Clone, Copy, Default)]
pub struct ForwardedRequest;

fn forbidden(message: &str) -> Error {
    Error::new(message).extend_with(|_, e| {
        e.set("code", Value::from("FORBIDDEN"));
    })
}

/// The federation context; `FORBIDDEN` when this server was built without one.
pub(crate) fn federation<'a>(ctx: &'a Context<'_>) -> Result<&'a FederationContext, Error> {
    ctx.data::<FederationContext>()
        .map_err(|_| forbidden("server-to-server features are not configured"))
}

/// The federation context of a relay; `FORBIDDEN` on a server that only hosts, or for a
/// forwarded request (the person's relay is elsewhere).
pub(crate) fn relay<'a>(ctx: &'a Context<'_>) -> Result<&'a FederationContext, Error> {
    let federation = federation(ctx)?;
    if !federation.relay_role {
        return Err(forbidden("this server is not a relay"));
    }
    if ctx.data::<ForwardedRequest>().is_ok() {
        return Err(forbidden("not available to remote members"));
    }
    Ok(federation)
}

/// The federation context of a server running the relay role, which holds the direct space;
/// forwarded requests included.
pub(crate) fn direct_space<'a>(ctx: &'a Context<'_>) -> Result<&'a FederationContext, Error> {
    let federation = federation(ctx)?;
    if !federation.relay_role {
        return Err(forbidden("this server is not a relay"));
    }
    Ok(federation)
}

/// Whether the caller may start a conversation with `user_ids` in `server_id`, a DM when
/// `direct_message`. In the direct space: `FORBIDDEN` when one is on a server the operator's
/// outbound DM setting does not admit, or a local one's DM settings refuse the caller;
/// `VALIDATION` while the caller's account is in probation and the conversation would send a DM
/// request to a stranger among them. There and in a workspace's DMs, `VALIDATION` when the
/// caller has a principal id but no profile card and one of them is a local user with a
/// principal id, and a conversation that files a DM request answers `FORBIDDEN` when its
/// recipient's request inbox is full and
/// `RATE_LIMITED` (with `retryAfterSeconds`) when the caller would file more requests than they
/// may per day. Everything else, and everything on a server without server-to-server traffic,
/// is allowed.
pub(crate) async fn ensure_may_request(
    ctx: &Context<'_>,
    actor: UserId,
    server_id: rim_core::server::model::ServerId,
    user_ids: Vec<UserId>,
    direct_message: bool,
) -> Result<(), Error> {
    let Ok(federation) = ctx.data::<FederationContext>() else {
        return Ok(());
    };
    if user_ids.is_empty() {
        return Ok(());
    }
    federation
        .host
        .ensure_may_request(
            rim_services::Actor::new(actor),
            server_id,
            user_ids,
            direct_message,
        )
        .await
        .map_err(map_service_error)
}

/// `FORBIDDEN` on a server that runs only the relay role: it creates no workspaces.
pub(crate) fn ensure_may_create_workspace(ctx: &Context<'_>) -> Result<(), Error> {
    match ctx.data::<FederationContext>() {
        Ok(federation) if !federation.host_role => Err(forbidden(
            "this server is a relay and hosts no new workspaces",
        )),
        _ => Ok(()),
    }
}

/// `FORBIDDEN` when a person from another server sets their display name here: a remote
/// member's display name comes from their profile card. Their avatar, status text and bio are
/// stored per host and may be set.
pub(crate) async fn ensure_may_set_display_name(
    ctx: &Context<'_>,
    user_id: UserId,
) -> Result<(), Error> {
    let Ok(federation) = ctx.data::<FederationContext>() else {
        return Ok(());
    };
    let remote = federation
        .host
        .is_remote_user(user_id)
        .await
        .map_err(map_service_error)?;
    if remote {
        return Err(forbidden(
            "a remote member's display name comes from their profile card",
        ));
    }
    Ok(())
}

/// Mutations that change an account rather than a space, which a person from another server
/// runs at their own relay, never here.
pub const ACCOUNT_MUTATIONS: &[&str] = &[
    "login",
    "logout",
    "logoutOtherSessions",
    "requestPasswordReset",
    "resetPassword",
    "keyLoginChallenge",
    "loginWithKey",
    "signupWithInvite",
    "signupWithKey",
    "enrollIdentityKey",
    "addDeviceKey",
    "revokeDeviceKey",
    "rotateIdentityKey",
    "publishProfileCard",
    "createServer",
    "startDeviceLink",
    "sendDeviceLinkOffer",
    "sendDeviceLinkAnswer",
    "completeDeviceLink",
    "cancelDeviceLink",
    "addContact",
    "removeContact",
    "acceptDmRequest",
    "ignoreDmRequest",
    "blockPrincipal",
    "unblockPrincipal",
    "reportDmRequest",
    "ackRelayItems",
    "leaveRelay",
    "remoteJoinChallenge",
    "joinRemoteWorkspace",
    "renewDelegations",
    "requestAccountDeletion",
    "cancelAccountDeletion",
    "setMyDmSettings",
    "setMyWorkspaceDmSetting",
];

/// The first account mutation the operation `operation_name` selects runs at its top level,
/// through fragments at any depth; or why the request cannot be cleared: a parse failure, or a
/// mutation whose root fields cannot all be found.
pub fn refused_account_mutation(query: &str, operation_name: Option<&str>) -> Option<String> {
    let fields = match parse_root_fields(query, operation_name) {
        Ok(fields) => fields,
        Err(e) => return Some(e.to_string()),
    };
    if !fields.may_mutate() {
        return None;
    }
    if let Classification::Incomplete(reason) = &fields.classification {
        return Some(reason.to_string());
    }
    fields
        .fields
        .into_iter()
        .find(|name| ACCOUNT_MUTATIONS.contains(&name.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_mutations_are_found_under_aliases_and_fragments() {
        assert_eq!(
            refused_account_mutation(
                "mutation { x: publishProfileCard(card: {payload: \"\", signature: \"\"}) { version } }",
                None
            ),
            Some("publishProfileCard".to_string())
        );
        assert_eq!(
            refused_account_mutation(
                "mutation { ...F } fragment F on Mutation { ... on Mutation { createServer(input: {name: \"x\"}) { id } } }",
                None
            ),
            Some("createServer".to_string())
        );
        assert_eq!(
            refused_account_mutation("mutation($id: ID!) { deleteMessage(messageId: $id) }", None),
            None
        );
        assert_eq!(refused_account_mutation("query { login }", None), None);
        assert!(refused_account_mutation("mutation {", None).is_some());
    }

    #[test]
    fn the_selected_operation_decides_and_an_unclear_mutation_is_refused() {
        let query = "mutation Space { deleteMessage(messageId: \"m\") } mutation Account { ...A } fragment A on Mutation { ...B } fragment B on Mutation { leaveRelay }";
        assert_eq!(refused_account_mutation(query, Some("Space")), None);
        assert_eq!(
            refused_account_mutation(query, Some("Account")),
            Some("leaveRelay".to_string())
        );
        assert_eq!(
            refused_account_mutation(query, None),
            Some("no operation is selected".to_string())
        );
        assert_eq!(
            refused_account_mutation(
                "mutation { deleteMessage(messageId: \"m\") ...Missing }",
                None
            ),
            Some("unknown fragment Missing".to_string())
        );
        assert_eq!(
            refused_account_mutation("query Q { me { id } } mutation M { leaveRelay }", Some("Q")),
            None
        );
    }
}
