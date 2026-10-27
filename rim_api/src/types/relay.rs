//! GraphQL types of the relay: identities from other servers, contacts, the request inbox and
//! the queue of host events, and of the host: stream heads.

use async_graphql::*;
use chrono::{DateTime, Utc};
use rim_core::relay::model::DmRequestStatus as CoreDmRequestStatus;
use rim_services::host::dto::{ConversationPredecessorDto, StreamHeadDto};
use rim_services::relay::dto::{
    BlockedPrincipalDto, ContactDto, DmRequestDto, IdentityDevice, RelayInfoDto, RelayItemDto,
    RelayItemPageDto, RemoteIdentityDto, RemoteServerDto, SignedJson,
};

use super::SignedStatement;

fn statement(s: SignedJson) -> SignedStatement {
    SignedStatement {
        payload: s.payload,
        signature: s.signature,
    }
}

/// This server as its users' relay.
#[derive(SimpleObject, Clone, Debug)]
pub struct RelayInfo {
    /// This server's id (`rims1_…`).
    pub server_id: String,
    /// `host`, `relay` or both.
    pub roles: Vec<String>,
    /// The space holding the DMs and groups this relay's users start with anyone.
    pub direct_space_id: ID,
}

impl From<RelayInfoDto> for RelayInfo {
    fn from(dto: RelayInfoDto) -> Self {
        Self {
            server_id: dto.server_id,
            roles: dto.roles,
            direct_space_id: ID(dto.direct_space_id.to_string()),
        }
    }
}

/// Another server, pinned through this relay.
#[derive(SimpleObject, Clone, Debug)]
pub struct RemoteServer {
    pub server_id: String,
    pub origin: String,
    /// The discovery document as JSON text, for the app to verify and pin.
    pub discovery: String,
}

impl From<RemoteServerDto> for RemoteServer {
    fn from(dto: RemoteServerDto) -> Self {
        Self {
            server_id: dto.server_id,
            origin: dto.origin,
            discovery: dto.discovery,
        }
    }
}

/// A device of a principal from another server.
#[derive(SimpleObject, Clone, Debug)]
pub struct RemotePrincipalDevice {
    pub device_id: ID,
    pub certificate: SignedStatement,
    pub revocation: Option<SignedStatement>,
    /// Whether the device uploaded key packages recently enough to be added to groups.
    pub mls_active: bool,
}

impl From<IdentityDevice> for RemotePrincipalDevice {
    fn from(d: IdentityDevice) -> Self {
        Self {
            device_id: ID(d.device_id.to_string()),
            certificate: statement(d.certificate),
            revocation: d.revocation.map(statement),
            mls_active: d.mls_active,
        }
    }
}

/// A principal's identity as their relay serves it. The app verifies the log against the
/// principal id, each certificate against the log and the card against the head.
#[derive(SimpleObject, Clone, Debug)]
pub struct RemoteIdentity {
    pub principal_id: String,
    pub key_log: Vec<SignedStatement>,
    pub devices: Vec<RemotePrincipalDevice>,
    pub card: Option<SignedStatement>,
    /// Server id of the relay that served it.
    pub served_by: String,
}

impl From<RemoteIdentityDto> for RemoteIdentity {
    fn from(dto: RemoteIdentityDto) -> Self {
        let doc = dto.document;
        Self {
            principal_id: doc.principal_id,
            key_log: doc.key_log.into_iter().map(statement).collect(),
            devices: doc.devices.into_iter().map(Into::into).collect(),
            card: doc.card.map(statement),
            served_by: dto.served_by,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Contact {
    pub principal_id: String,
    pub relay_urls: Vec<String>,
    pub added_at: DateTime<Utc>,
}

impl From<ContactDto> for Contact {
    fn from(dto: ContactDto) -> Self {
        Self {
            principal_id: dto.principal_id,
            relay_urls: dto.relay_urls,
            added_at: dto.added_at,
        }
    }
}

/// A principal the caller blocked: its DM requests are ignored and its invites and key
/// package claims refused.
#[derive(SimpleObject, Clone, Debug)]
pub struct BlockedPrincipal {
    /// The blocked principal's id.
    pub principal_id: String,
    /// The account on this server that holds the principal; null when none does.
    pub user_id: Option<ID>,
    /// That account's username; null when no account here holds the principal.
    pub username: Option<String>,
    /// That account's display name; null when it has none or no account here holds the
    /// principal.
    pub display_name: Option<String>,
    /// When the caller blocked the principal.
    pub blocked_at: DateTime<Utc>,
}

impl From<BlockedPrincipalDto> for BlockedPrincipal {
    fn from(dto: BlockedPrincipalDto) -> Self {
        Self {
            principal_id: dto.principal_id,
            user_id: dto.user_id.map(|id| ID(id.to_string())),
            username: dto.username,
            display_name: dto.display_name,
            blocked_at: dto.blocked_at,
        }
    }
}

#[derive(Enum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum DmRequestStatus {
    Pending,
    Accepted,
    Ignored,
}

impl From<CoreDmRequestStatus> for DmRequestStatus {
    fn from(status: CoreDmRequestStatus) -> Self {
        match status {
            CoreDmRequestStatus::Pending => Self::Pending,
            CoreDmRequestStatus::Accepted => Self::Accepted,
            CoreDmRequestStatus::Ignored => Self::Ignored,
        }
    }
}

impl From<DmRequestStatus> for CoreDmRequestStatus {
    fn from(status: DmRequestStatus) -> Self {
        match status {
            DmRequestStatus::Pending => Self::Pending,
            DmRequestStatus::Accepted => Self::Accepted,
            DmRequestStatus::Ignored => Self::Ignored,
        }
    }
}

/// A host event queued for the caller, signed by its host.
#[derive(SimpleObject, Clone, Debug)]
pub struct RelayItem {
    pub id: ID,
    pub host_server_id: String,
    pub workspace_id: ID,
    pub envelope: SignedStatement,
}

impl From<RelayItemDto> for RelayItem {
    fn from(dto: RelayItemDto) -> Self {
        Self {
            id: ID(dto.id.to_string()),
            host_server_id: dto.host_server_id,
            workspace_id: ID(dto.workspace_id.to_string()),
            envelope: statement(dto.envelope),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct RelayItemPage {
    pub items: Vec<RelayItem>,
    pub has_more: bool,
}

impl From<RelayItemPageDto> for RelayItemPage {
    fn from(dto: RelayItemPageDto) -> Self {
        Self {
            items: dto.items.into_iter().map(Into::into).collect(),
            has_more: dto.has_more,
        }
    }
}

/// A conversation started by someone outside the caller's contacts.
#[derive(SimpleObject, Clone, Debug)]
pub struct DmRequest {
    pub id: ID,
    pub from_principal_id: String,
    /// The sender's profile card as the host sent it.
    pub card: SignedStatement,
    pub host_server_id: String,
    pub conversation_id: ID,
    /// Up to three items of the conversation, held while the request is pending.
    pub held_items: Vec<RelayItem>,
    pub status: DmRequestStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl From<DmRequestDto> for DmRequest {
    fn from(dto: DmRequestDto) -> Self {
        Self {
            id: ID(dto.id.to_string()),
            from_principal_id: dto.from_principal_id,
            card: statement(dto.card),
            host_server_id: dto.host_server_id,
            conversation_id: ID(dto.conversation_id.to_string()),
            held_items: dto.held_items.into_iter().map(Into::into).collect(),
            status: dto.status.into(),
            created_at: dto.created_at,
            expires_at: dto.expires_at,
        }
    }
}

/// The current `seq` of a host event stream, as a decimal string.
#[derive(SimpleObject, Clone, Debug)]
pub struct StreamHead {
    pub stream: String,
    pub seq: String,
}

impl From<StreamHeadDto> for StreamHead {
    fn from(dto: StreamHeadDto) -> Self {
        Self {
            stream: dto.stream,
            seq: dto.seq.to_string(),
        }
    }
}

/// The conversation a successor replaces, on the host that held it.
#[derive(SimpleObject, Clone, Debug)]
pub struct ConversationPredecessor {
    pub host_server_id: String,
    pub conversation_id: ID,
}

impl From<ConversationPredecessorDto> for ConversationPredecessor {
    fn from(dto: ConversationPredecessorDto) -> Self {
        Self {
            host_server_id: dto.host_server_id,
            conversation_id: ID(dto.conversation_id.to_string()),
        }
    }
}

/// Someone from another server to start a conversation with.
#[derive(InputObject, Clone, Debug)]
pub struct RemoteMemberInput {
    pub principal_id: String,
    /// Their relays, tried in order to fetch their identity.
    pub relay_urls: Vec<String>,
}

/// The conversation a successor replaces.
#[derive(InputObject, Clone, Debug)]
pub struct PredecessorInput {
    pub host_server_id: String,
    pub conversation_id: ID,
}
