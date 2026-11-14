use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use rim_identity::PublicKey;
use rim_s2s::SharedIdentity;
use rim_s2s::http_sig::{
    IncomingResponse, OutgoingRequest, RIM_ORIGIN, RIM_REQUEST_NONCE, RequestBinding, new_nonce,
    parse_response, sign_request,
};
use rim_services::s2s::dto::{
    DiscoveryRequester, PeerKeysDto, PeerLookup, S2sClientError, S2sMethod, S2sRequest, S2sResponse,
};
use rim_services::s2s::service::{PeerService, S2sClient};
use url::Url;

use crate::s2s::net::{NetError, check_url_host, http_client, read_limited};

/// Response headers handed back with the answer: the host-response pair for the forward path,
/// and `Retry-After` for a peer's `429`.
const KEPT_RESPONSE_HEADERS: &[&str] = &["rim-host-response", "rim-host-signature", "retry-after"];

/// How long a signed request stays valid.
const REQUEST_LIFETIME_SECS: i64 = 300;

/// Settings of the signing client.
#[derive(Debug, Clone)]
pub struct S2sClientConfig {
    /// This server's origin, sent as `Rim-Origin`; `None` while server-to-server traffic is
    /// off.
    pub own_origin: Option<String>,
    pub allow_http: bool,
    pub allow_private: bool,
    pub timeout: Duration,
    pub max_response_bytes: usize,
}

/// Signs requests with this server's key, sends them to a pinned peer's `endpoints.s2s`, and
/// accepts only answers signed by the peer's pinned key for that request.
pub struct DefaultS2sClient {
    identity: SharedIdentity,
    peers: Arc<dyn PeerService>,
    http: reqwest::Client,
    config: S2sClientConfig,
}

impl DefaultS2sClient {
    pub fn new(
        identity: SharedIdentity,
        peers: Arc<dyn PeerService>,
        config: S2sClientConfig,
    ) -> Result<Self, NetError> {
        Ok(Self {
            http: http_client(config.allow_private, config.timeout)?,
            identity,
            peers,
            config,
        })
    }

    async fn ready_keys(
        &self,
        server_id: &str,
        key_seq: Option<u32>,
    ) -> Result<PeerKeysDto, S2sClientError> {
        match self
            .peers
            .resolve(server_id, None, key_seq, DiscoveryRequester::Server)
            .await
        {
            Ok(PeerLookup::Ready(keys)) => Ok(keys),
            Ok(PeerLookup::Unknown) => Err(S2sClientError::UnknownPeer),
            Ok(PeerLookup::Forked) => Err(S2sClientError::PeerForked),
            Ok(PeerLookup::Blocked) => Err(S2sClientError::PeerBlocked),
            Err(e) => Err(S2sClientError::Transport(e.to_string())),
        }
    }
}

/// Check that `url`, parsed from `endpoint` followed by `path`, has as its path exactly the
/// endpoint's path followed by the part of `path` before any `?`.
///
/// # Errors
/// `BadRequest` when the parser changed the path: it resolved a `.` or `..` segment, read a
/// backslash as a slash, dropped a tab or line break, percent-encoded a character or ended the
/// path at a `#`.
fn check_literal_path(endpoint: &str, path: &str, url: &Url) -> Result<(), S2sClientError> {
    let base = Url::parse(endpoint).map_err(|e| S2sClientError::BadRequest(e.to_string()))?;
    let base_path = match base.path() {
        "/" if !endpoint.ends_with('/') => "",
        p => p,
    };
    let named = path.split_once('?').map_or(path, |(p, _)| p);
    if url.path() == format!("{base_path}{named}") {
        Ok(())
    } else {
        Err(S2sClientError::BadRequest(
            "the path does not stay as written".into(),
        ))
    }
}

fn header<'a>(response: &'a reqwest::Response, name: &str) -> Option<&'a str> {
    response.headers().get(name).and_then(|v| v.to_str().ok())
}

#[async_trait]
impl S2sClient for DefaultS2sClient {
    async fn send(&self, request: S2sRequest) -> Result<S2sResponse, S2sClientError> {
        let origin = self
            .config
            .own_origin
            .as_deref()
            .ok_or(S2sClientError::Disabled)?;
        let peer_id = request.peer_server_id.as_str();
        let keys = self.ready_keys(peer_id, None).await?;
        let endpoint = keys
            .s2s_endpoint
            .as_deref()
            .ok_or_else(|| S2sClientError::BadRequest("the peer has no s2s endpoint".into()))?;
        if !request.path.starts_with('/') {
            return Err(S2sClientError::BadRequest("path must start with /".into()));
        }
        let url = Url::parse(&format!("{endpoint}{}", request.path))
            .map_err(|e| S2sClientError::BadRequest(e.to_string()))?;
        check_literal_path(endpoint, &request.path, &url)?;
        match url.scheme() {
            "https" => {}
            "http" if self.config.allow_http => {}
            _ => {
                return Err(S2sClientError::BadRequest(
                    "the peer's endpoint is not https".into(),
                ));
            }
        }
        check_url_host(&url, self.config.allow_private)
            .map_err(|e| S2sClientError::Transport(e.to_string()))?;

        let identity = self.identity.current();
        let key_id = identity.key_id();
        let nonce = new_nonce().map_err(|e| S2sClientError::Transport(format!("entropy: {e}")))?;
        let created = Utc::now().timestamp();
        let content_type = request
            .content_type
            .clone()
            .unwrap_or_else(|| "application/json".to_string());
        let body = match request.method {
            S2sMethod::Post => Some(request.body.as_slice()),
            S2sMethod::Get => None,
        };
        let target_uri = url.as_str().to_string();
        let signed = sign_request(
            &identity.signer,
            &OutgoingRequest {
                method: request.method.as_str(),
                target_uri: &target_uri,
                body,
                content_type: &content_type,
                rim_origin: origin,
                created,
                expires: created + REQUEST_LIFETIME_SECS,
                nonce: &nonce,
                key_id: &key_id,
            },
        )
        .ok_or_else(|| S2sClientError::BadRequest("cannot sign the request".into()))?;

        let mut builder = match request.method {
            S2sMethod::Get => self.http.get(url),
            S2sMethod::Post => self
                .http
                .post(url)
                .header(reqwest::header::CONTENT_TYPE, &content_type)
                .body(request.body.clone()),
        }
        .header(RIM_ORIGIN, origin)
        .header("signature-input", &signed.signature_input)
        .header("signature", &signed.signature);
        if let Some(digest) = &signed.content_digest {
            builder = builder.header("content-digest", digest);
        }
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let response = builder
            .send()
            .await
            .map_err(|e| S2sClientError::Transport(e.to_string()))?;

        let status = response.status().as_u16();
        let owned = |name: &str| header(&response, name).map(str::to_string);
        let kept_headers: Vec<(String, String)> = KEPT_RESPONSE_HEADERS
            .iter()
            .filter_map(|name| owned(name).map(|value| ((*name).to_string(), value)))
            .collect();
        let (digest, response_type, echoed, input, signature) = (
            owned("content-digest"),
            owned("content-type"),
            owned(RIM_REQUEST_NONCE),
            owned("signature-input"),
            owned("signature"),
        );
        let response_body = read_limited(response, self.config.max_response_bytes)
            .await
            .map_err(|e| S2sClientError::Transport(e.to_string()))?;

        let binding = RequestBinding {
            method: request.method.as_str(),
            target_uri: &target_uri,
            content_digest: signed.content_digest.as_deref(),
            nonce: &nonce,
        };
        let refused = |reason: &str| S2sClientError::BadResponseSignature(reason.to_string());
        let pending = parse_response(
            &IncomingResponse {
                status,
                content_digest: digest.as_deref(),
                content_type: response_type.as_deref(),
                rim_request_nonce: echoed.as_deref(),
                signature_input: input.as_deref(),
                signature: signature.as_deref(),
            },
            &binding,
            Utc::now().timestamp(),
        )
        .map_err(|r| refused(r.as_str()))?;
        if pending.key_id.server_id != peer_id {
            return Err(refused("keyid names another server"));
        }
        let seq = pending.key_id.seq;
        let key_text = match keys.key_for_seq(seq) {
            Some(key) => key.to_string(),
            None if seq > keys.head_seq => self
                .ready_keys(peer_id, Some(seq))
                .await?
                .key_for_seq(seq)
                .map(str::to_string)
                .ok_or_else(|| refused("keyid"))?,
            None => return Err(refused("keyid")),
        };
        let key = PublicKey::parse(&key_text, "peer key").map_err(|_| refused("keyid"))?;
        pending
            .check_digest(&response_body)
            .map_err(|r| refused(r.as_str()))?;
        pending.verify(&key).map_err(|r| refused(r.as_str()))?;

        if status == 401 {
            let _ = self.peers.refresh(peer_id).await;
        }
        Ok(S2sResponse {
            status,
            content_type: response_type.unwrap_or_default(),
            body: response_body,
            key_seq: seq,
            headers: kept_headers,
        })
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use rim_s2s::{
        Endpoints, Limits, Registration, RegistrationMode, Role, ServerDescriptor, ServerIdentity,
        ServerKeyState, ServerSigner, sign_descriptor, sign_inception, timestamp,
    };
    use rim_services::s2s::service::MockPeerService;

    use super::*;

    const PEER: &str = "rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c";

    /// A server identity from a fixed seed.
    fn identity() -> SharedIdentity {
        let signer = ServerSigner::from_seed([5; 32]);
        let recovery = ServerSigner::from_seed([105; 32]);
        let incepted_at = DateTime::from_timestamp(1_767_225_600, 0).expect("2026-01-01");
        let inception = sign_inception(&signer, &recovery, incepted_at);
        let state = ServerKeyState::from_inception(&inception).expect("inception");
        let content = ServerDescriptor::new(
            state.server_id.clone(),
            0,
            1,
            vec![Role::Relay],
            Endpoints {
                graphql: "https://relay.example/api".into(),
                websocket: "wss://relay.example/api/ws".into(),
                s2s: Some("https://relay.example/s2s/v1".into()),
            },
            vec![79],
            Limits {
                max_body_bytes: 8_388_608,
                clock_skew_seconds: 300,
                max_deliver_items: 256,
            },
            Registration {
                mode: RegistrationMode::Invite,
                pow_bits: None,
            },
            None,
            timestamp(Utc::now()),
        );
        let descriptor = sign_descriptor(&signer, &content).expect("descriptor");
        SharedIdentity::new(ServerIdentity {
            signer,
            state,
            log: vec![inception],
            descriptor,
            descriptor_content: content,
        })
    }

    /// A client whose one peer, [`PEER`], has its s2s endpoint at `endpoint`. Private
    /// addresses are refused, so a request to a loopback endpoint stops before the network.
    fn client(endpoint: &str) -> DefaultS2sClient {
        let endpoint = endpoint.to_string();
        let mut peers = MockPeerService::new();
        peers.expect_resolve().returning(move |server_id, _, _, _| {
            let keys = PeerKeysDto {
                server_id: server_id.to_string(),
                origin: "https://127.0.0.1".to_string(),
                head_seq: 0,
                head_key: "ed25519:k".to_string(),
                previous_key: None,
                s2s_endpoint: Some(endpoint.clone()),
            };
            Box::pin(async move { Ok(PeerLookup::Ready(keys)) })
        });
        DefaultS2sClient::new(
            identity(),
            Arc::new(peers),
            S2sClientConfig {
                own_origin: Some("https://relay.example".to_string()),
                allow_http: false,
                allow_private: false,
                timeout: Duration::from_secs(1),
                max_response_bytes: 1024,
            },
        )
        .expect("client")
    }

    async fn send(endpoint: &str, path: &str) -> Result<S2sResponse, S2sClientError> {
        client(endpoint)
            .send(S2sRequest {
                peer_server_id: PEER.to_string(),
                method: S2sMethod::Get,
                path: path.to_string(),
                body: Vec::new(),
                content_type: None,
                headers: Vec::new(),
            })
            .await
    }

    #[tokio::test]
    async fn a_path_a_url_parser_would_rewrite_is_refused_before_it_is_signed() {
        for path in [
            "/forward/../outbox",
            "/forward/%2e%2e/outbox",
            "/forward/a/%2E%2E/.%2e/outbox?after=0",
            "/forward/./x",
            "/forward/..\\outbox",
            "/forward/.\t./outbox",
            "/forward/../../../api",
            "/forward/x#y",
        ] {
            let result = send("https://127.0.0.1/s2s/v1", path).await;
            assert!(
                matches!(result, Err(S2sClientError::BadRequest(_))),
                "{path}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_literal_path_passes_the_check() {
        for (endpoint, path) in [
            ("https://127.0.0.1/s2s/v1", "/forward/x/y?z=1"),
            ("https://127.0.0.1/s2s/v1", "/forward/a..b/.x?q=/../"),
            ("https://127.0.0.1", "/forward/x"),
            ("https://127.0.0.1/s2s/v1/", "/ping"),
        ] {
            let result = send(endpoint, path).await;
            assert!(
                matches!(&result, Err(S2sClientError::Transport(e)) if e.contains("127.0.0.1")),
                "{endpoint}{path}: {result:?}"
            );
        }
    }
}
