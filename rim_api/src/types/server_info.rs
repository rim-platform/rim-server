//! GraphQL type describing the running server build.

use async_graphql::*;

/// The major version of this GraphQL API. Raised when a field or argument is removed or
/// changes meaning, so a client can tell it is older than the server it talks to.
pub const API_VERSION: i32 = 1;

/// The running server build and the API version it speaks.
#[derive(SimpleObject, Clone, Debug)]
pub struct ServerInfo {
    /// The server's release version, e.g. `0.1.0`.
    pub version: String,
    /// The major version of this GraphQL API.
    pub api_version: i32,
}

impl ServerInfo {
    /// This build's version and API version.
    pub fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            api_version: API_VERSION,
        }
    }
}
