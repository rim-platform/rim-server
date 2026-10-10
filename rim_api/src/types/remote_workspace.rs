//! GraphQL types of workspaces hosted on another server: joining through the relay, the
//! relay's subscriptions, and where a device revocation stands at each host.

use async_graphql::*;
use chrono::{DateTime, Utc};
use rim_core::relay::model::FanoutStatus;
use rim_services::relay::dto::SignedJson;
use rim_services::remote_workspace::dto::{
    JoinChallengeDto, RemoteJoinResultDto, RemoteSubscriptionDto, RevocationFanoutDto,
};

use super::{SignedStatement, SignedStatementInput};

/// A login challenge from a host, fetched through the relay, for a device to sign with the
/// host's server id as `audience`.
#[derive(SimpleObject, Clone, Debug)]
pub struct RemoteJoinChallenge {
    pub challenge_id: ID,
    pub nonce: String,
    pub expires_at: DateTime<Utc>,
}

impl From<JoinChallengeDto> for RemoteJoinChallenge {
    fn from(dto: JoinChallengeDto) -> Self {
        Self {
            challenge_id: ID(dto.challenge_id.to_string()),
            nonce: dto.nonce,
            expires_at: dto.expires_at,
        }
    }
}

/// A workspace joined on another host.
#[derive(SimpleObject, Clone, Debug)]
pub struct RemoteJoinResult {
    pub host_server_id: String,
    pub workspace_id: ID,
    /// The member's user id at the host.
    pub user_id: ID,
    /// `owner`, `moderator`, `member` or `guest`.
    pub role: String,
}

impl From<RemoteJoinResultDto> for RemoteJoinResult {
    fn from(dto: RemoteJoinResultDto) -> Self {
        Self {
            host_server_id: dto.host_server_id,
            workspace_id: ID(dto.workspace_id.to_string()),
            user_id: ID(dto.user_id.to_string()),
            role: dto.role,
        }
    }
}

/// The relay's subscription to a workspace on another host, or a delegation the host refused.
#[derive(SimpleObject, Clone, Debug)]
pub struct RemoteSubscription {
    pub host_server_id: String,
    pub workspace_id: ID,
    /// `active`, `lapsed` or `ended`; `refused` for a delegation the host did not accept.
    pub status: String,
    /// When the host stops honouring the delegation that keeps it active.
    pub expires_at: Option<DateTime<Utc>>,
    /// Why a delegation was refused (`not_member`, `expired`, `relay_key`, …).
    pub reason: Option<String>,
}

impl From<RemoteSubscriptionDto> for RemoteSubscription {
    fn from(dto: RemoteSubscriptionDto) -> Self {
        Self {
            host_server_id: dto.host_server_id,
            workspace_id: ID(dto.workspace_id.to_string()),
            status: dto.status,
            expires_at: dto.expires_at,
            reason: dto.reason,
        }
    }
}

/// Where a revocation's delivery to one host stands.
#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum RevocationFanoutStatus {
    Pending,
    Acked,
    GaveUp,
}

impl From<FanoutStatus> for RevocationFanoutStatus {
    fn from(status: FanoutStatus) -> Self {
        match status {
            FanoutStatus::Pending => Self::Pending,
            FanoutStatus::Acked => Self::Acked,
            FanoutStatus::GaveUp => Self::GaveUp,
        }
    }
}

/// A device revocation at one host.
#[derive(SimpleObject, Clone, Debug)]
pub struct RevocationFanout {
    pub host_server_id: String,
    pub status: RevocationFanoutStatus,
    /// The host's signed `revocation-ack`, checked against its key.
    pub ack: Option<SignedStatement>,
    pub attempts: i32,
}

impl From<RevocationFanoutDto> for RevocationFanout {
    fn from(dto: RevocationFanoutDto) -> Self {
        Self {
            host_server_id: dto.host_server_id,
            status: dto.status.into(),
            ack: dto.ack.map(|a| SignedStatement {
                payload: a.payload,
                signature: a.signature,
            }),
            attempts: dto.attempts,
        }
    }
}

/// Join a workspace on another host through this relay.
#[derive(InputObject, Clone, Debug)]
pub struct JoinRemoteWorkspaceInput {
    pub host_server_id: String,
    /// The host's origin, from the invite link.
    pub host_origin: String,
    /// The invite token; null for a current member.
    pub invite_token: Option<String>,
    /// A `login` statement over a `remoteJoinChallenge`, naming the host's server id.
    pub proof: SignedStatementInput,
    /// A delegation of the signing device for this relay, the host and the workspace.
    pub delegation: SignedStatementInput,
}

/// A signed statement input as it travels between servers.
pub(crate) fn signed_json(input: SignedStatementInput) -> SignedJson {
    SignedJson {
        payload: input.payload,
        signature: input.signature,
    }
}
