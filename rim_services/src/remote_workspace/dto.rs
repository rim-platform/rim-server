use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rim_core::relay::model::FanoutStatus;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::relay::dto::SignedJson;

/// A login challenge a host issues for a join (`POST /s2s/v1/join/challenge`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinChallengeDto {
    pub challenge_id: Uuid,
    pub nonce: String,
    pub expires_at: DateTime<Utc>,
    /// The workspace the invite names, for the delegation the device signs next; null when the
    /// request named no invite.
    #[serde(default)]
    pub workspace_id: Option<Uuid>,
}

/// `POST /s2s/v1/join/challenge` body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JoinChallengeRequest {
    /// The invite the person is joining with; it is looked up, not used.
    #[serde(default)]
    pub invite_token: Option<String>,
}

/// A device of the person joining: its certificate and any revocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JoinDevice {
    pub certificate: SignedJson,
    #[serde(default)]
    pub revocation: Option<SignedJson>,
}

/// `POST /s2s/v1/join`: a relay presents a person joining a workspace here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JoinRequest {
    /// The workspace invite; `None` for a current member.
    pub invite_token: Option<String>,
    pub key_log: Vec<SignedJson>,
    pub devices: Vec<JoinDevice>,
    #[serde(default)]
    pub card: Option<SignedJson>,
    /// A `login` statement over a challenge from the host, naming the host's server id.
    pub proof: SignedJson,
    pub delegation: SignedJson,
    /// The person's username at the relay presenting them, outside every signed statement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

/// The host's answer to a join.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinAnswer {
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    /// The member's role, as `server_member` spells it.
    pub role: String,
    /// The current `seq` of each stream the member sees (`w` and their `u:` stream).
    pub stream_heads: BTreeMap<String, i64>,
}

/// `POST /s2s/v1/subscriptions` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionsRequest {
    pub delegations: Vec<SignedJson>,
}

/// Whether a host took one presented delegation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionResult {
    /// The delegation's workspace, when it could be read.
    pub workspace_id: Option<Uuid>,
    /// `active` or `refused`.
    pub status: String,
    /// When the host stops honouring it, to the second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl SubscriptionResult {
    pub fn active(workspace_id: Uuid, expires_at: DateTime<Utc>) -> Self {
        Self {
            workspace_id: Some(workspace_id),
            status: "active".to_string(),
            expires_at: Some(expires_at.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
            reason: None,
        }
    }

    pub fn refused(workspace_id: Option<Uuid>, reason: &str) -> Self {
        Self {
            workspace_id,
            status: "refused".to_string(),
            expires_at: None,
            reason: Some(reason.to_string()),
        }
    }

    pub fn is_active(&self) -> bool {
        self.status == "active"
    }
}

/// `POST /s2s/v1/subscriptions` answer: one entry per delegation, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionsAnswer {
    pub subscriptions: Vec<SubscriptionResult>,
}

/// One subscription a relay cancels: a user's, for one workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelEntry {
    pub workspace_id: Uuid,
    pub principal_id: String,
}

/// `POST /s2s/v1/subscriptions/cancel` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelRequest {
    pub subscriptions: Vec<CancelEntry>,
}

/// Whether a person has a socket open at their relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresenceUpdate {
    pub principal_id: String,
    pub online: bool,
}

/// `POST /s2s/v1/presence` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresenceRequest {
    pub updates: Vec<PresenceUpdate>,
}

/// `POST /s2s/v1/revocations` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevocationRequest {
    pub principal_id: String,
    pub revocation: SignedJson,
}

/// `POST /s2s/v1/revocations` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationAnswer {
    pub ack: SignedJson,
}

/// What `joinRemoteWorkspace` sends through the relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteJoinInput {
    pub host_server_id: String,
    pub host_origin: String,
    pub invite_token: Option<String>,
    pub proof: SignedJson,
    pub delegation: SignedJson,
}

/// A workspace joined on another host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteJoinResultDto {
    pub host_server_id: String,
    pub workspace_id: Uuid,
    /// The member's user id at the host.
    pub user_id: Uuid,
    pub role: String,
}

/// A relay's subscription to a workspace on another host, or a delegation the host refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSubscriptionDto {
    pub host_server_id: String,
    pub workspace_id: Uuid,
    /// `active`, `lapsed` or `ended`; `refused` for a delegation that was not accepted.
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    /// Why a delegation was refused.
    pub reason: Option<String>,
}

/// Where a device's revocation stands at one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationFanoutDto {
    pub host_server_id: String,
    pub status: FanoutStatus,
    /// The host's acknowledgement, verified against its key.
    pub ack: Option<SignedJson>,
    pub attempts: i32,
}

/// What a remote-workspace cleanup pass changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteWorkspaceCleanup {
    pub ended_delegations: u64,
    pub lapsed_subscriptions: u64,
}
