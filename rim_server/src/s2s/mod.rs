//! The server-to-server transport: the discovery document and the signed API under
//! `/s2s/v1/`. A non-GraphQL transport that depends on `rim_services` traits; the composition
//! root hands it the concrete services.

pub(crate) mod discovery;
pub(crate) mod forward;
mod guard;
mod roles;

use std::collections::HashSet;
use std::sync::Arc;

use rim_infra_services::KeyedRateLimiter;
use rim_s2s::SharedIdentity;
use rim_services::host::service::HostService;
use rim_services::relay::service::RelayService;
use rim_services::remote_workspace::service::{WorkspaceHostService, WorkspaceRelayService};
use rim_services::s2s::service::{PeerService, S2sClient, S2sNonceService};

use crate::config::ServerRole;

pub(crate) use guard::handle;

/// The server-to-server settings resolved from the configuration at boot.
#[derive(Debug, Clone)]
pub struct S2sSettings {
    pub role: ServerRole,
    /// This server's origin (`Rim-Origin`, the base of `@target-uri`); `None` while
    /// server-to-server traffic is off.
    pub origin: Option<String>,
    pub denied_peers: HashSet<String>,
    /// Relays allowed on host endpoints; empty allows any.
    pub allowed_relays: HashSet<String>,
    pub max_body_bytes: usize,
}

/// What the server-to-server routes share.
#[derive(Clone)]
pub struct S2sState {
    pub identity: SharedIdentity,
    pub peers: Arc<dyn PeerService>,
    pub nonces: Arc<dyn S2sNonceService>,
    /// Requests per peer per minute.
    pub rate_limiter: Arc<KeyedRateLimiter>,
    pub settings: Arc<S2sSettings>,
    /// Sends signed requests to peers.
    pub client: Arc<dyn S2sClient>,
    /// This server as its users' relay.
    pub relay: Arc<dyn RelayService>,
    /// This server as the host of spaces with remote members.
    pub host: Arc<dyn HostService>,
    /// This server as the host of workspaces with members whose relay is elsewhere.
    pub workspace_host: Arc<dyn WorkspaceHostService>,
    /// This server as the relay of members of workspaces hosted elsewhere.
    pub workspace_relay: Arc<dyn WorkspaceRelayService>,
}
