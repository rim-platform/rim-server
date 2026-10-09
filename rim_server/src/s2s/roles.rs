//! The server-to-server endpoints, the role each belongs to, and which roles serve them.

use rim_s2s::Role;

use crate::config::ServerRole;

/// A server-to-server endpoint below `/s2s/v1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Endpoint {
    /// `GET /ping`: the answering server's id and roles.
    Ping,
    /// `GET /principals/<principalId>`: a local user's identity.
    Principal,
    /// `POST /identity`: an identity pushed for a principal held here as remote.
    Identity,
    /// `POST /key-packages/claim`: key packages of a local user's devices.
    KeyPackageClaim,
    /// `POST /deliver`: host events for local users.
    Deliver,
    /// `GET /outbox`: what this host queued for the calling relay.
    Outbox,
    /// `GET|POST /forward/<path>`: a device-signed request run as its user and device.
    Forward,
}

/// Which roles serve an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndpointRole {
    /// Every server.
    All,
    /// Hosts, and relays for their direct space.
    Host,
    /// Relays only; a host answers `503 role_disabled`.
    Relay,
}

impl Endpoint {
    /// The endpoint for a method and a path below `/s2s/v1`.
    pub(crate) fn route(method: &str, path: &str) -> Option<Self> {
        if let Some(principal) = path.strip_prefix("/principals/") {
            return (method == "GET" && !principal.is_empty() && !principal.contains('/'))
                .then_some(Self::Principal);
        }
        if path.starts_with("/forward/") {
            return matches!(method, "GET" | "POST").then_some(Self::Forward);
        }
        match (method, path) {
            ("GET", "/ping") => Some(Self::Ping),
            ("POST", "/identity") => Some(Self::Identity),
            ("POST", "/key-packages/claim") => Some(Self::KeyPackageClaim),
            ("POST", "/deliver") => Some(Self::Deliver),
            ("GET", "/outbox") => Some(Self::Outbox),
            _ => None,
        }
    }

    pub(crate) fn role(self) -> EndpointRole {
        match self {
            Self::Ping => EndpointRole::All,
            Self::Principal | Self::KeyPackageClaim | Self::Deliver => EndpointRole::Relay,
            Self::Identity | Self::Outbox | Self::Forward => EndpointRole::Host,
        }
    }

    /// Whether `RIM_HOST_ALLOWED_RELAYS` applies to the endpoint.
    pub(crate) fn host_allow_list_applies(self) -> bool {
        self == Self::Forward
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
    fn endpoints_route_by_method_and_path() {
        assert_eq!(
            Endpoint::route("GET", "/principals/rimp1_x"),
            Some(Endpoint::Principal)
        );
        assert_eq!(Endpoint::route("POST", "/principals/rimp1_x"), None);
        assert_eq!(Endpoint::route("GET", "/principals/"), None);
        assert_eq!(Endpoint::route("GET", "/principals/a/b"), None);
        assert_eq!(
            Endpoint::route("POST", "/identity"),
            Some(Endpoint::Identity)
        );
        assert_eq!(
            Endpoint::route("POST", "/key-packages/claim"),
            Some(Endpoint::KeyPackageClaim)
        );
        assert_eq!(Endpoint::route("POST", "/deliver"), Some(Endpoint::Deliver));
        assert_eq!(Endpoint::route("GET", "/deliver"), None);
        assert_eq!(Endpoint::route("GET", "/outbox"), Some(Endpoint::Outbox));
        assert_eq!(
            Endpoint::route("POST", "/forward/api"),
            Some(Endpoint::Forward)
        );
        assert_eq!(
            Endpoint::route("GET", "/forward/api/attachments/x/download"),
            Some(Endpoint::Forward)
        );
    }

    #[test]
    fn the_role_table() {
        let relay_only = [
            Endpoint::Principal,
            Endpoint::KeyPackageClaim,
            Endpoint::Deliver,
        ];
        for endpoint in relay_only {
            assert_eq!(endpoint.role(), EndpointRole::Relay, "{endpoint:?}");
            assert!(!serves(ServerRole::Host, endpoint.role()), "{endpoint:?}");
            assert!(serves(ServerRole::Relay, endpoint.role()), "{endpoint:?}");
            assert!(serves(ServerRole::Both, endpoint.role()), "{endpoint:?}");
        }
        for endpoint in [Endpoint::Identity, Endpoint::Outbox, Endpoint::Forward] {
            assert_eq!(endpoint.role(), EndpointRole::Host, "{endpoint:?}");
            for role in [ServerRole::Relay, ServerRole::Host, ServerRole::Both] {
                assert!(serves(role, endpoint.role()), "{endpoint:?} on {role:?}");
            }
        }
        assert!(Endpoint::Forward.host_allow_list_applies());
        assert!(!Endpoint::Identity.host_allow_list_applies());
        assert!(!Endpoint::Outbox.host_allow_list_applies());
        assert!(!Endpoint::Deliver.host_allow_list_applies());
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
