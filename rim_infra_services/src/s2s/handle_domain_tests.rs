use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Utc;
use rim_s2s::{
    Endpoints, Limits, Registration, RegistrationMode, Role, ServerDescriptor, ServerKeyState,
    ServerSigner, sign_descriptor, sign_inception,
};

use super::*;
use crate::s2s::net::NetError;

/// Serves a fixed document per origin and records the origins asked for.
#[derive(Default)]
struct Served {
    docs: HashMap<String, Vec<u8>>,
    asked: Mutex<Vec<String>>,
}

impl Served {
    fn with(mut self, origin: &str, doc: Vec<u8>) -> Self {
        self.docs.insert(origin.to_string(), doc);
        self
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().expect("lock").clone()
    }
}

#[async_trait]
impl DiscoveryFetcher for Served {
    async fn fetch(&self, origin: &str) -> Result<Vec<u8>, NetError> {
        self.asked.lock().expect("lock").push(origin.to_string());
        self.docs
            .get(origin)
            .cloned()
            .ok_or_else(|| NetError::Transport("unreachable".into()))
    }
}

/// The discovery document of the server with key seed `seed` at `origin`, declaring
/// `handle_domain`. Returns the server id and the document.
fn server(seed: u8, origin: &str, handle_domain: Option<&str>) -> (String, Vec<u8>) {
    let signer = ServerSigner::from_seed([seed; 32]);
    let inception = sign_inception(
        &signer,
        &ServerSigner::from_seed([seed.wrapping_add(100); 32]),
        Utc::now(),
    );
    let state = ServerKeyState::from_inception(&inception).expect("inception");
    let descriptor = sign_descriptor(
        &signer,
        &ServerDescriptor::new(
            state.server_id.clone(),
            0,
            1,
            vec![Role::Host, Role::Relay],
            Endpoints {
                graphql: format!("{origin}/api"),
                websocket: format!("{}/api/ws", origin.replace("https", "wss")),
                s2s: Some(format!("{origin}/s2s/v1")),
            },
            vec![79],
            Limits {
                max_body_bytes: 8 * 1024 * 1024,
                clock_skew_seconds: 300,
                max_deliver_items: 256,
            },
            Registration {
                mode: RegistrationMode::Invite,
                pow_bits: None,
            },
            handle_domain.map(str::to_string),
            "2026-10-09T00:00:00Z".into(),
        ),
    )
    .expect("descriptor");
    let stmt = |s: &rim_identity::SignedStatement| serde_json::json!({"payload": s.payload, "signature": s.signature});
    let doc = serde_json::to_vec(&serde_json::json!({
        "rim": "server-discovery",
        "keyLog": [stmt(&inception)],
        "descriptor": stmt(&descriptor),
    }))
    .expect("json");
    (state.server_id, doc)
}

fn delegate(to: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "delegate": to })).expect("json")
}

const APEX: &str = "https://rim-platform.org";
const RELAY: &str = "https://relay.rim-platform.org";
const DOMAIN: &str = "rim-platform.org";

#[tokio::test]
async fn a_delegation_is_followed_once_to_a_server_declaring_the_domain() {
    let (relay_id, relay_doc) = server(1, RELAY, Some(DOMAIN));
    let served = Served::default()
        .with(APEX, delegate(RELAY))
        .with(RELAY, relay_doc);
    let resolved = resolve_handle_domain(&served, DOMAIN, false)
        .await
        .expect("resolves");
    assert_eq!(resolved.origin, RELAY);
    assert_eq!(resolved.discovery.state.server_id, relay_id);
    assert_eq!(served.asked(), vec![APEX.to_string(), RELAY.to_string()]);
}

#[tokio::test]
async fn a_domain_serving_its_own_server_resolves_without_delegation() {
    let (id, doc) = server(2, APEX, Some(DOMAIN));
    let served = Served::default().with(APEX, doc);
    let resolved = resolve_handle_domain(&served, DOMAIN, false)
        .await
        .expect("resolves");
    assert_eq!(resolved.origin, APEX);
    assert_eq!(resolved.discovery.state.server_id, id);
}

#[tokio::test]
async fn the_delegated_server_must_declare_the_domain_it_was_reached_from() {
    for declared in [None, Some("other.example")] {
        let (_, relay_doc) = server(3, RELAY, declared);
        let served = Served::default()
            .with(APEX, delegate(RELAY))
            .with(RELAY, relay_doc);
        assert!(matches!(
            resolve_handle_domain(&served, DOMAIN, false).await,
            Err(ResolveError::Refused(_))
        ));
    }
}

#[tokio::test]
async fn a_second_delegation_is_not_followed() {
    let served = Served::default()
        .with(APEX, delegate(RELAY))
        .with(RELAY, delegate("https://third.example"));
    assert!(matches!(
        resolve_handle_domain(&served, DOMAIN, false).await,
        Err(ResolveError::Refused(_))
    ));
    assert_eq!(served.asked().len(), 2);
}

#[tokio::test]
async fn a_delegate_must_be_an_https_origin() {
    for to in [
        "http://relay.rim-platform.org",
        "https://relay.rim-platform.org/path",
        "relay.rim-platform.org",
        "https://user@relay.rim-platform.org",
    ] {
        let served = Served::default().with(APEX, delegate(to));
        assert!(
            matches!(
                resolve_handle_domain(&served, DOMAIN, false).await,
                Err(ResolveError::Refused(_))
            ),
            "{to}"
        );
        assert_eq!(served.asked().len(), 1, "{to}");
    }
}

#[tokio::test]
async fn a_delegation_with_other_fields_or_junk_is_refused() {
    for body in [
        serde_json::to_vec(&serde_json::json!({"delegate": RELAY, "extra": 1})).expect("json"),
        b"not json".to_vec(),
    ] {
        let served = Served::default().with(APEX, body);
        assert!(matches!(
            resolve_handle_domain(&served, DOMAIN, false).await,
            Err(ResolveError::Refused(_))
        ));
    }
}

#[tokio::test]
async fn an_unreachable_document_is_unavailable_and_a_bad_domain_is_never_fetched() {
    let served = Served::default().with(APEX, delegate(RELAY));
    assert!(matches!(
        resolve_handle_domain(&served, DOMAIN, false).await,
        Err(ResolveError::Unavailable(_))
    ));
    let served = Served::default();
    assert!(matches!(
        resolve_handle_domain(&served, "Rim-Platform.org", false).await,
        Err(ResolveError::Refused(_))
    ));
    assert!(served.asked().is_empty());
}

#[tokio::test]
async fn a_peer_at_its_own_domain_is_verified_without_a_fetch() {
    let served = Served::default();
    assert_eq!(
        check_handle_domain(&served, RELAY, "rims1_x", "relay.rim-platform.org", false).await,
        DomainCheck::Verified
    );
    assert!(served.asked().is_empty());
}

#[tokio::test]
async fn a_peer_claiming_a_domain_is_verified_only_when_the_domain_points_at_it() {
    let (relay_id, relay_doc) = server(4, RELAY, Some(DOMAIN));
    let served = Served::default()
        .with(APEX, delegate(RELAY))
        .with(RELAY, relay_doc);
    assert_eq!(
        check_handle_domain(&served, RELAY, &relay_id, DOMAIN, false).await,
        DomainCheck::Verified
    );
    // Another server claiming the same domain is refused: the domain points at the relay.
    let (other_id, _) = server(5, "https://evil.example", Some(DOMAIN));
    assert!(matches!(
        check_handle_domain(&served, "https://evil.example", &other_id, DOMAIN, false).await,
        DomainCheck::Refused(_)
    ));
    // Nothing answers at the domain: try again later.
    let served = Served::default();
    assert!(matches!(
        check_handle_domain(&served, RELAY, &relay_id, DOMAIN, false).await,
        DomainCheck::Unavailable(_)
    ));
}
