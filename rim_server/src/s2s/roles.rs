//! The server-to-server endpoints, the role each belongs to, and which roles serve them.

use rim_s2s::Role;

use crate::config::ServerRole;

/// A server-to-server endpoint below `/s2s/v1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Endpoint {
    /// `GET /ping`: the answering server's id and roles.
    Ping,
}

/// Which roles serve an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndpointRole {
    /// Every server.
    All,
    /// Hosts, and relays for their direct space.
    Host,
    /// Relays only; a host answers `503 role_disabled`. No such endpoint exists yet.
    #[allow(dead_code)]
    Relay,
}

impl Endpoint {
    /// The endpoint for a method and a path below `/s2s/v1`.
    pub(crate) fn route(method: &str, path: &str) -> Option<Self> {
        match (method, path) {
            ("GET", "/ping") => Some(Self::Ping),
            _ => None,
        }
    }

    pub(crate) fn role(self) -> EndpointRole {
        match self {
            Self::Ping => EndpointRole::All,
        }
    }

    /// Whether `RIM_HOST_ALLOWED_RELAYS` applies to the endpoint.
    pub(crate) fn host_allow_list_applies(self) -> bool {
        self.role() == EndpointRole::Host
    }
}

/// Whether a server running `role` serves an endpoint of `endpoint_role`.
pub(crate) fn serves(role: ServerRole, endpoint_role: EndpointRole) -> bool {
    match endpoint_role {
        EndpointRole::All => true,
        // A relay hosts its direct space, so it serves host endpoints for that space.
        EndpointRole::Host => role.runs(Role::Host) || role.runs(Role::Relay),
        EndpointRole::Relay => role.runs(Role::Relay),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_is_served_by_every_role() {
        assert_eq!(Endpoint::route("GET", "/ping"), Some(Endpoint::Ping));
        assert_eq!(Endpoint::route("POST", "/ping"), None);
        assert_eq!(Endpoint::route("GET", "/pong"), None);
        for role in [ServerRole::Relay, ServerRole::Host, ServerRole::Both] {
            assert!(serves(role, Endpoint::Ping.role()));
        }
        assert!(!Endpoint::Ping.host_allow_list_applies());
    }

    #[test]
    fn relay_endpoints_are_disabled_on_a_host() {
        assert!(!serves(ServerRole::Host, EndpointRole::Relay));
        assert!(serves(ServerRole::Relay, EndpointRole::Relay));
        assert!(serves(ServerRole::Both, EndpointRole::Relay));
        for role in [ServerRole::Relay, ServerRole::Host, ServerRole::Both] {
            assert!(serves(role, EndpointRole::Host));
        }
    }
}
