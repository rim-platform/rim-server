use async_graphql::*;
use rim_services::Actor;
use rim_services::remote_workspace::dto::{JoinChallengeRequest, RemoteJoinInput};

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::federation::{federation, relay};
use crate::types::remote_workspace::signed_json;
use crate::types::{
    JoinRemoteWorkspaceInput, RemoteJoinChallenge, RemoteJoinResult, RemoteSubscription,
    SignedStatement, SignedStatementInput,
};

#[derive(Default)]
pub(crate) struct RemoteWorkspaceMutations;

#[Object]
impl RemoteWorkspaceMutations {
    /// Pin the host at `hostOrigin` as `hostServerId` and fetch a login challenge from it for a
    /// join, with the workspace `inviteToken` names (the invite is not used), or the workspace,
    /// its name and the link's mode `publicLinkToken` names. At most one of the two.
    async fn remote_join_challenge(
        &self,
        ctx: &Context<'_>,
        host_server_id: String,
        host_origin: String,
        invite_token: Option<String>,
        public_link_token: Option<String>,
    ) -> Result<RemoteJoinChallenge> {
        let actor = require_actor(ctx)?;
        Ok(relay(ctx)?
            .workspace_relay
            .remote_join_challenge(
                Actor::new(actor),
                host_server_id,
                host_origin,
                JoinChallengeRequest {
                    invite_token,
                    public_link_token,
                },
            )
            .await
            .map_err(map_service_error)?
            .into())
    }

    /// Join a workspace on another host: the relay presents the caller's identity, proof and
    /// delegation, and records the subscription. Through a public link that takes join
    /// requests, the host files a request instead (`outcome: REQUESTED`).
    async fn join_remote_workspace(
        &self,
        ctx: &Context<'_>,
        input: JoinRemoteWorkspaceInput,
    ) -> Result<RemoteJoinResult> {
        let actor = require_actor(ctx)?;
        Ok(relay(ctx)?
            .workspace_relay
            .join_remote_workspace(
                Actor::new(actor),
                RemoteJoinInput {
                    host_server_id: input.host_server_id,
                    host_origin: input.host_origin,
                    invite_token: input.invite_token,
                    public_link_token: input.public_link_token,
                    message: input.message,
                    proof: signed_json(input.proof),
                    delegation: signed_json(input.delegation),
                },
            )
            .await
            .map_err(map_service_error)?
            .into())
    }

    /// Present fresh delegations of the caller's devices to their hosts; one entry per
    /// delegation, in order.
    async fn renew_delegations(
        &self,
        ctx: &Context<'_>,
        delegations: Vec<SignedStatementInput>,
    ) -> Result<Vec<RemoteSubscription>> {
        let actor = require_actor(ctx)?;
        Ok(relay(ctx)?
            .workspace_relay
            .renew_delegations(
                Actor::new(actor),
                delegations.into_iter().map(signed_json).collect(),
            )
            .await
            .map_err(map_service_error)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Apply a device revocation on this host directly, as an app does when a host did not
    /// acknowledge it through the relay. Needs no session: it acts only on the revocation's
    /// signature, and attempts are limited per IP. Returns the host's signed acknowledgement.
    async fn apply_remote_revocation(
        &self,
        ctx: &Context<'_>,
        revocation: SignedStatementInput,
    ) -> Result<SignedStatement> {
        let ack = federation(ctx)?
            .workspace_host
            .apply_revocation_directly(crate::auth_context::remote_ip(ctx), signed_json(revocation))
            .await
            .map_err(map_service_error)?;
        Ok(SignedStatement {
            payload: ack.payload,
            signature: ack.signature,
        })
    }
}
