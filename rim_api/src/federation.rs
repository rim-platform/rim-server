//! What resolvers need from this server's relay and host roles: the services, the roles this
//! server runs, and the account mutations a person from another server may not run here.

use std::sync::Arc;

use async_graphql::parser::types::{ExecutableDocument, OperationType, Selection, SelectionSet};
use async_graphql::{Context, Error, ErrorExtensions, Value};
use rim_core::user::model::UserId;
use rim_services::host::service::HostService;
use rim_services::relay::dto::RelayInfoDto;
use rim_services::relay::service::RelayService;
use rim_services::remote_workspace::service::{WorkspaceHostService, WorkspaceRelayService};

use crate::errors::map_service_error;

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

/// `VALIDATION` while the caller's account is in probation and a conversation in the direct
/// space would send a DM request to a stranger among `user_ids`. Elsewhere, and on a server
/// without the direct space, it allows everything.
pub(crate) async fn ensure_may_request(
    ctx: &Context<'_>,
    actor: UserId,
    server_id: rim_core::server::model::ServerId,
    user_ids: Vec<UserId>,
) -> Result<(), Error> {
    let Ok(federation) = ctx.data::<FederationContext>() else {
        return Ok(());
    };
    if server_id != federation.host.direct_space_id() || user_ids.is_empty() {
        return Ok(());
    }
    federation
        .host
        .ensure_may_request(rim_services::Actor::new(actor), user_ids)
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
];

fn refused_in(document: &ExecutableDocument, set: &SelectionSet, depth: usize) -> Option<String> {
    if depth > 16 {
        return Some("fragments nest too deep".to_string());
    }
    for selection in &set.items {
        let found = match &selection.node {
            Selection::Field(field) => {
                let name = field.node.name.node.as_str();
                ACCOUNT_MUTATIONS.contains(&name).then(|| name.to_string())
            }
            Selection::InlineFragment(fragment) => {
                refused_in(document, &fragment.node.selection_set.node, depth + 1)
            }
            Selection::FragmentSpread(spread) => document
                .fragments
                .get(&spread.node.fragment_name.node)
                .and_then(|f| refused_in(document, &f.node.selection_set.node, depth + 1)),
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// The first account mutation a query names at its top level, or a parse failure's message.
pub fn refused_account_mutation(query: &str) -> Option<String> {
    let document = match async_graphql::parser::parse_query(query) {
        Ok(document) => document,
        Err(e) => return Some(e.to_string()),
    };
    document
        .operations
        .iter()
        .filter(|(_, op)| op.node.ty == OperationType::Mutation)
        .find_map(|(_, op)| refused_in(&document, &op.node.selection_set.node, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_mutations_are_found_under_aliases_and_fragments() {
        assert_eq!(
            refused_account_mutation(
                "mutation { x: publishProfileCard(card: {payload: \"\", signature: \"\"}) { version } }"
            ),
            Some("publishProfileCard".to_string())
        );
        assert_eq!(
            refused_account_mutation(
                "mutation { ...F } fragment F on Mutation { ... on Mutation { createServer(input: {name: \"x\"}) { id } } }"
            ),
            Some("createServer".to_string())
        );
        assert_eq!(
            refused_account_mutation("mutation($id: ID!) { deleteMessage(messageId: $id) }"),
            None
        );
        assert_eq!(refused_account_mutation("query { login }"), None);
        assert!(refused_account_mutation("mutation {").is_some());
    }
}
