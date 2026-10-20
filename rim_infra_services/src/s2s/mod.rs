//! Server identity and server-to-server traffic: the server key and descriptor, pinned peers,
//! handle domains, request nonces, and the signing client other servers are called through.

pub mod client;
pub(crate) mod handle_domain;
pub mod net;
pub mod nonces;
pub mod peers;
pub mod server_key;

pub use client::{DefaultS2sClient, S2sClientConfig};
pub use nonces::DefaultS2sNonceService;
pub use peers::{DefaultPeerService, DiscoveryFetcher, HttpDiscoveryFetcher};
