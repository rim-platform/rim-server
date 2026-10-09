use chrono::{DateTime, Utc};

/// What a pinned peer currently signs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerKeysDto {
    pub server_id: String,
    pub origin: String,
    pub head_seq: u32,
    /// Tagged text of the head server key.
    pub head_key: String,
    /// The key before the head, while it is still accepted: 24 hours after this server learned
    /// of a `signer: server` rotation, never after a `signer: recovery` one.
    pub previous_key: Option<String>,
    /// The peer's `endpoints.s2s`, from its verified descriptor.
    pub s2s_endpoint: Option<String>,
}

impl PeerKeysDto {
    /// The tagged key a signature at `seq` must verify under, if that seq is accepted.
    pub fn key_for_seq(&self, seq: u32) -> Option<&str> {
        if seq == self.head_seq {
            Some(&self.head_key)
        } else if seq.checked_add(1) == Some(self.head_seq) {
            self.previous_key.as_deref()
        } else {
            None
        }
    }
}

/// The result of looking a peer up for a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerLookup {
    Ready(PeerKeysDto),
    /// Not pinned, and it could not be pinned from the given origin.
    Unknown,
    /// The peer's key log stopped extending its pin; nothing is served until an operator
    /// resets it.
    Forked,
    /// Refused by an operator.
    Blocked,
}

/// A pinned peer, as an operator or a resolver sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerDto {
    pub server_id: String,
    pub origin: String,
    pub status: String,
    pub head_seq: u32,
    pub descriptor_version: u32,
    /// The descriptor payload as signed.
    pub descriptor_payload: String,
    pub fetched_at: DateTime<Utc>,
}

/// An HTTP method of the server-to-server API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S2sMethod {
    Get,
    Post,
}

impl S2sMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// A request to a peer's server-to-server API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S2sRequest {
    pub peer_server_id: String,
    pub method: S2sMethod,
    /// The path below the peer's `endpoints.s2s`, starting with `/`, with any query.
    pub path: String,
    /// The body of a `POST`; ignored for `GET`.
    pub body: Vec<u8>,
    /// `Content-Type` of a `POST`; `application/json` when `None`.
    pub content_type: Option<String>,
}

/// A peer's answer whose signature verified against its pin. Error statuses are answers too:
/// the body is `{"error": "<code>", "message": "…"}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S2sResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    /// The seq of the peer key that signed the response.
    pub key_seq: u32,
}

impl S2sResponse {
    /// The `error` code of an error body.
    pub fn error_code(&self) -> Option<String> {
        let value: serde_json::Value = serde_json::from_slice(&self.body).ok()?;
        value.get("error")?.as_str().map(str::to_string)
    }
}

/// Why a request to a peer produced no verified answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum S2sClientError {
    /// Server-to-server traffic is off on this server.
    #[error("server-to-server traffic is off")]
    Disabled,
    #[error("peer is not pinned")]
    UnknownPeer,
    #[error("peer's key log forked")]
    PeerForked,
    #[error("peer is blocked")]
    PeerBlocked,
    /// The peer has no server-to-server endpoint, or the request cannot be built.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Connection, timeout, size or protocol failure; retrying may help.
    #[error("transport failure: {0}")]
    Transport(String),
    /// The response was unsigned or its signature did not verify: a transport failure, never
    /// an answer.
    #[error("response signature refused: {0}")]
    BadResponseSignature(String),
}
