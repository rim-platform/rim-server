//! What resolvers need from this server's relay and host roles: the services, the roles this
//! server runs, and the account mutations a person from another server may not run here.

use std::sync::Arc;

use async_graphql::parser::types::{ExecutableDocument, OperationType, Selection, SelectionSet};
use async_graphql::{Context, Error, ErrorExtensions, Value};
use rim_services::host::service::HostService;
use rim_services::relay::dto::RelayInfoDto;
use rim_services::relay::service::RelayService;

/// The relay and host services and this server's roles, injected into the schema.
#[derive(Clone)]
pub struct FederationContext {
    pub relay: Arc<dyn RelayService>,
    pub host: Arc<dyn HostService>,
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

/// `FORBIDDEN` on a server that runs only the relay role: it creates no workspaces.
pub(crate) fn ensure_may_create_workspace(ctx: &Context<'_>) -> Result<(), Error> {
    match ctx.data::<FederationContext>() {
        Ok(federation) if !federation.host_role => Err(forbidden(
            "this server is a relay and hosts no new workspaces",
        )),
        _ => Ok(()),
    }
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
    "updateMyProfile",
    "createServer",
    "createIngestToken",
    "revokeIngestToken",
    "createInstanceInvite",
    "revokeInstanceInvite",
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
