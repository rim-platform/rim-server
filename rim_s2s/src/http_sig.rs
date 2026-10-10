//! The HTTP message signature profile (RFC 9421) servers sign their requests and responses
//! with: one label, a fixed list of covered components, fixed parameters, Ed25519 from the
//! tagged server key, and `Content-Digest` (RFC 9530) over bodies.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use rim_identity::PublicKey;
use sfv::{
    BareItem, DictSerializer, Dictionary, Integer, KeyRef, ListEntry, ListSerializer, Parser,
    StringRef,
};
use sha2::{Digest, Sha256};

use crate::error::SignatureRefusal;
use crate::key_log::is_server_id;
use crate::signer::ServerSigner;

/// The label the profile signs under; other labels are ignored.
pub const SIGNATURE_LABEL: &str = "sig1";

/// The `tag` parameter of every signature in the profile.
pub const SIGNATURE_TAG: &str = "rim-s2s-v1";

/// Longest a signature may be valid, and how far ahead of the receiver's clock `created` may
/// be, in seconds.
pub const MAX_WINDOW_SECS: i64 = 300;

/// How long a receiver remembers a request nonce, counted from `created`, in seconds.
pub const NONCE_RETENTION_SECS: i64 = 600;

/// Bytes of randomness in a request nonce.
pub const NONCE_LEN: usize = 16;

/// The header carrying the sender's origin.
pub const RIM_ORIGIN: &str = "rim-origin";

/// The response header echoing the request's nonce.
pub const RIM_REQUEST_NONCE: &str = "rim-request-nonce";

/// A covered component: an HTTP field name in lowercase or a derived component such as
/// `@method`, optionally taken from the request (`;req`) when signing a response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Component {
    pub name: &'static str,
    pub req: bool,
}

impl Component {
    pub const fn new(name: &'static str) -> Self {
        Self { name, req: false }
    }

    pub const fn req(name: &'static str) -> Self {
        Self { name, req: true }
    }
}

const METHOD: Component = Component::new("@method");
const TARGET_URI: Component = Component::new("@target-uri");
const CONTENT_DIGEST: Component = Component::new("content-digest");
const CONTENT_TYPE: Component = Component::new("content-type");
const ORIGIN: Component = Component::new(RIM_ORIGIN);
const STATUS: Component = Component::new("@status");
const REQUEST_NONCE: Component = Component::new(RIM_REQUEST_NONCE);
const REQ_METHOD: Component = Component::req("@method");
const REQ_TARGET_URI: Component = Component::req("@target-uri");
const REQ_CONTENT_DIGEST: Component = Component::req("content-digest");

const REQUEST_WITH_BODY: &[Component] = &[METHOD, TARGET_URI, CONTENT_DIGEST, CONTENT_TYPE, ORIGIN];
const REQUEST_WITHOUT_BODY: &[Component] = &[METHOD, TARGET_URI, ORIGIN];
const RESPONSE_TO_BODY: &[Component] = &[
    STATUS,
    CONTENT_DIGEST,
    CONTENT_TYPE,
    REQUEST_NONCE,
    REQ_METHOD,
    REQ_TARGET_URI,
    REQ_CONTENT_DIGEST,
];
const RESPONSE_TO_NO_BODY: &[Component] = &[
    STATUS,
    CONTENT_DIGEST,
    CONTENT_TYPE,
    REQUEST_NONCE,
    REQ_METHOD,
    REQ_TARGET_URI,
];

const REQUEST_PARAMS: &[&str] = &["created", "expires", "nonce", "keyid", "alg", "tag"];
const RESPONSE_PARAMS: &[&str] = &["created", "keyid", "alg", "tag"];

/// A signature parameter value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Param {
    Int(i64),
    Str(String),
}

/// A `keyid`: the signing server and the seq of its key log entry, `"<serverId>#<seq>"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyId {
    pub server_id: String,
    pub seq: u32,
}

impl KeyId {
    pub fn parse(s: &str) -> Option<Self> {
        let (server_id, seq) = s.split_once('#')?;
        if !is_server_id(server_id)
            || seq.is_empty()
            || !seq.bytes().all(|b| b.is_ascii_digit())
            || (seq.len() > 1 && seq.starts_with('0'))
        {
            return None;
        }
        Some(Self {
            server_id: server_id.to_string(),
            seq: seq.parse().ok()?,
        })
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.server_id, self.seq)
    }
}

/// `Content-Digest` for `body`: `sha-256=:<padded base64 SHA-256>:`.
pub fn content_digest(body: &[u8]) -> String {
    let digest: [u8; 32] = Sha256::digest(body).into();
    let mut ser = DictSerializer::new();
    if let Ok(key) = KeyRef::from_str("sha-256") {
        let _ = ser.bare_item(key, &digest[..]);
    }
    ser.finish().unwrap_or_default()
}

/// Whether a `Content-Digest` field carries a `sha-256` digest equal to `body`'s. Other
/// algorithms are ignored; a field without `sha-256` does not match.
pub fn digest_matches(field: &str, body: &[u8]) -> bool {
    let Ok(dict) = Parser::new(field).parse::<Dictionary>() else {
        return false;
    };
    let Some(ListEntry::Item(item)) = dict.get("sha-256") else {
        return false;
    };
    let digest: [u8; 32] = Sha256::digest(body).into();
    item.bare_item.as_byte_sequence() == Some(&digest[..])
}

/// A fresh request nonce: 16 random bytes, unpadded base64url.
pub fn new_nonce() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; NONCE_LEN];
    getrandom::fill(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn is_nonce(s: &str) -> bool {
    URL_SAFE_NO_PAD
        .decode(s)
        .is_ok_and(|bytes| bytes.len() == NONCE_LEN)
}

fn component_id(component: Component) -> String {
    let Ok(name) = StringRef::from_str(component.name) else {
        return String::new();
    };
    let ser = sfv::ItemSerializer::new().bare_item(name);
    match (component.req, KeyRef::from_str("req")) {
        (true, Ok(req)) => ser.parameter(req, true).finish(),
        _ => ser.finish(),
    }
}

/// The `@signature-params` value, which is also the `Signature-Input` member's value: the
/// covered components as an inner list, then the parameters in order. `None` when a value
/// cannot be a structured field (a parameter string outside printable ASCII).
pub fn signature_params(components: &[Component], params: &[(&str, Param)]) -> Option<String> {
    let mut ser = ListSerializer::new();
    {
        let mut inner = ser.inner_list();
        for component in components {
            let item = inner.bare_item(StringRef::from_str(component.name).ok()?);
            if component.req {
                let _ = item.parameter(KeyRef::from_str("req").ok()?, true);
            }
        }
        let mut list_params = inner.finish();
        for (name, value) in params {
            let key = KeyRef::from_str(name).ok()?;
            list_params = match value {
                Param::Int(v) => list_params.parameter(key, Integer::try_from(*v).ok()?),
                Param::Str(v) => list_params.parameter(key, StringRef::from_str(v).ok()?),
            };
        }
    }
    ser.finish()
}

/// The signature base (RFC 9421 section 2.5): one `"<component>": <value>` line per covered
/// component, then `"@signature-params"`, joined by `\n` with no trailing newline.
pub fn signature_base(lines: &[(Component, &str)], signature_params: &str) -> String {
    let mut base = String::new();
    for (component, value) in lines {
        base.push_str(&component_id(*component));
        base.push_str(": ");
        base.push_str(value);
        base.push('\n');
    }
    base.push_str("\"@signature-params\": ");
    base.push_str(signature_params);
    base
}

fn signature_field(signature: &[u8]) -> String {
    let mut ser = DictSerializer::new();
    if let Ok(label) = KeyRef::from_str(SIGNATURE_LABEL) {
        let _ = ser.bare_item(label, signature);
    }
    ser.finish().unwrap_or_default()
}

/// The headers a signer adds to a request or response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHeaders {
    /// `Content-Digest` of the body. A request without a body has none.
    pub content_digest: Option<String>,
    pub signature_input: String,
    pub signature: String,
}

/// A request a server is about to send. `body` is `Some` exactly for `POST`.
#[derive(Debug, Clone)]
pub struct OutgoingRequest<'a> {
    pub method: &'a str,
    pub target_uri: &'a str,
    pub body: Option<&'a [u8]>,
    pub content_type: &'a str,
    pub rim_origin: &'a str,
    pub created: i64,
    pub expires: i64,
    pub nonce: &'a str,
    pub key_id: &'a KeyId,
}

/// Sign a request. The caller sends the returned headers with the request, and
/// `Content-Type` as given.
pub fn sign_request(
    signer: &ServerSigner,
    request: &OutgoingRequest<'_>,
) -> Option<SignatureHeaders> {
    let digest = request.body.map(content_digest);
    let (components, lines): (&[Component], Vec<(Component, &str)>) = match &digest {
        Some(digest) => (
            REQUEST_WITH_BODY,
            vec![
                (METHOD, request.method),
                (TARGET_URI, request.target_uri),
                (CONTENT_DIGEST, digest),
                (CONTENT_TYPE, request.content_type),
                (ORIGIN, request.rim_origin),
            ],
        ),
        None => (
            REQUEST_WITHOUT_BODY,
            vec![
                (METHOD, request.method),
                (TARGET_URI, request.target_uri),
                (ORIGIN, request.rim_origin),
            ],
        ),
    };
    let params = signature_params(
        components,
        &[
            ("created", Param::Int(request.created)),
            ("expires", Param::Int(request.expires)),
            ("nonce", Param::Str(request.nonce.to_string())),
            ("keyid", Param::Str(request.key_id.to_string())),
            ("alg", Param::Str(signer.algorithm().name().to_string())),
            ("tag", Param::Str(SIGNATURE_TAG.to_string())),
        ],
    )?;
    let base = signature_base(&lines, &params);
    Some(SignatureHeaders {
        content_digest: digest,
        signature_input: format!("{SIGNATURE_LABEL}={params}"),
        signature: signature_field(&signer.sign_bytes(base.as_bytes())),
    })
}

/// A request as the receiver sees it. Field values are the received ones, several lines of one
/// field joined with `", "`; `target_uri` is rebuilt from the receiver's public base URL.
#[derive(Debug, Clone, Default)]
pub struct IncomingRequest<'a> {
    pub method: &'a str,
    pub target_uri: &'a str,
    pub content_digest: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub rim_origin: Option<&'a str>,
    pub signature_input: Option<&'a str>,
    pub signature: Option<&'a str>,
}

/// A request whose signature fits the profile and its time window, ready to be checked
/// against the signing server's key.
#[derive(Debug, Clone)]
pub struct PendingRequest {
    pub key_id: KeyId,
    pub nonce: String,
    pub created: i64,
    pub expires: i64,
    pub rim_origin: String,
    /// The received `Content-Digest`, for a request with a body.
    pub content_digest: Option<String>,
    alg: String,
    base: String,
    signature: Vec<u8>,
}

impl PendingRequest {
    /// Check the body against the signed `Content-Digest`. A request without a body must have
    /// none.
    pub fn check_digest(&self, body: &[u8]) -> Result<(), SignatureRefusal> {
        match &self.content_digest {
            Some(field) if digest_matches(field, body) => Ok(()),
            None if body.is_empty() => Ok(()),
            _ => Err(SignatureRefusal::Digest),
        }
    }

    /// Check the signature with `key`, which must be of the algorithm `alg` names.
    pub fn verify(&self, key: &PublicKey) -> Result<(), SignatureRefusal> {
        verify_base(key, &self.alg, &self.base, &self.signature)
    }
}

fn verify_base(
    key: &PublicKey,
    alg: &str,
    base: &str,
    signature: &[u8],
) -> Result<(), SignatureRefusal> {
    if alg != key.algorithm().name() {
        return Err(SignatureRefusal::Algorithm);
    }
    key.verify_bytes(base.as_bytes(), signature)
        .map_err(|_| SignatureRefusal::Verify)
}

/// The profile's signature and its parameters, read from `Signature-Input` and `Signature`.
struct ParsedSignature {
    params: Vec<(String, BareItem)>,
    signature: Vec<u8>,
}

fn parse_signature(
    signature_input: Option<&str>,
    signature: Option<&str>,
    expected: &[Component],
) -> Result<ParsedSignature, SignatureRefusal> {
    let (Some(input), Some(signature)) = (signature_input, signature) else {
        return Err(SignatureRefusal::MissingHeader);
    };
    let inputs: Dictionary = Parser::new(input)
        .parse()
        .map_err(|_| SignatureRefusal::Malformed)?;
    let signatures: Dictionary = Parser::new(signature)
        .parse()
        .map_err(|_| SignatureRefusal::Malformed)?;
    let (Some(ListEntry::InnerList(list)), Some(ListEntry::Item(sig))) =
        (inputs.get(SIGNATURE_LABEL), signatures.get(SIGNATURE_LABEL))
    else {
        return Err(SignatureRefusal::MissingLabel);
    };
    let signature = sig
        .bare_item
        .as_byte_sequence()
        .filter(|_| sig.params.is_empty())
        .ok_or(SignatureRefusal::Malformed)?
        .to_vec();

    if list.items.len() != expected.len() {
        return Err(SignatureRefusal::Components);
    }
    for (item, component) in list.items.iter().zip(expected) {
        let name = item.bare_item.as_string().map(|s| s.as_str());
        let params_ok = if component.req {
            item.params.len() == 1
                && item.params.get("req").and_then(BareItem::as_boolean) == Some(true)
        } else {
            item.params.is_empty()
        };
        if name != Some(component.name) || !params_ok {
            return Err(SignatureRefusal::Components);
        }
    }
    Ok(ParsedSignature {
        params: list
            .params
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.clone()))
            .collect(),
        signature,
    })
}

/// Read parameters in exactly the expected order: integers for `created` and `expires`,
/// strings for the rest.
fn read_params(
    params: &[(String, BareItem)],
    expected: &[&str],
) -> Result<Vec<Param>, SignatureRefusal> {
    if params.len() != expected.len() {
        return Err(SignatureRefusal::Parameters);
    }
    params
        .iter()
        .zip(expected)
        .map(|((name, value), want)| {
            if name != want {
                return Err(SignatureRefusal::Parameters);
            }
            match *want {
                "created" | "expires" => value
                    .as_integer()
                    .map(|i| Param::Int(i64::from(i)))
                    .ok_or(SignatureRefusal::Parameters),
                _ => value
                    .as_string()
                    .map(|s| Param::Str(s.as_str().to_string()))
                    .ok_or(SignatureRefusal::Parameters),
            }
        })
        .collect()
}

fn int(p: &Param) -> i64 {
    match p {
        Param::Int(v) => *v,
        Param::Str(_) => 0,
    }
}

fn text(p: &Param) -> &str {
    match p {
        Param::Str(v) => v,
        Param::Int(_) => "",
    }
}

fn components_for_method(method: &str) -> Result<&'static [Component], SignatureRefusal> {
    match method {
        "POST" => Ok(REQUEST_WITH_BODY),
        "GET" => Ok(REQUEST_WITHOUT_BODY),
        _ => Err(SignatureRefusal::Components),
    }
}

/// Check a request's signature against the profile at `now` (Unix seconds): the label, exactly
/// the profile's components and parameters, `created` at most 300 s ahead, `expires` in the
/// future and at most 300 s after `created`, a well-formed nonce, `keyid` and tag. The key
/// lookup, digest and signature checks follow on the result.
pub fn parse_request(
    request: &IncomingRequest<'_>,
    now: i64,
) -> Result<PendingRequest, SignatureRefusal> {
    let components = components_for_method(request.method)?;
    let parsed = parse_signature(request.signature_input, request.signature, components)?;
    let params = read_params(&parsed.params, REQUEST_PARAMS)?;
    let (created, expires) = (int(&params[0]), int(&params[1]));
    let (nonce, keyid, alg, tag) = (
        text(&params[2]),
        text(&params[3]),
        text(&params[4]),
        text(&params[5]),
    );
    if tag != SIGNATURE_TAG || !is_nonce(nonce) {
        return Err(SignatureRefusal::Parameters);
    }
    if created > now + MAX_WINDOW_SECS
        || expires <= now
        || expires <= created
        || expires - created > MAX_WINDOW_SECS
    {
        return Err(SignatureRefusal::Clock);
    }
    let key_id = KeyId::parse(keyid).ok_or(SignatureRefusal::KeyId)?;
    let rim_origin = request
        .rim_origin
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .ok_or(SignatureRefusal::MissingHeader)?;

    let has_body = components.len() == REQUEST_WITH_BODY.len();
    let content_digest = request.content_digest.map(str::trim);
    let content_type = request.content_type.map(str::trim);
    let lines: Vec<(Component, &str)> = if has_body {
        let (Some(digest), Some(content_type)) = (content_digest, content_type) else {
            return Err(SignatureRefusal::MissingHeader);
        };
        vec![
            (METHOD, request.method),
            (TARGET_URI, request.target_uri),
            (CONTENT_DIGEST, digest),
            (CONTENT_TYPE, content_type),
            (ORIGIN, rim_origin),
        ]
    } else {
        vec![
            (METHOD, request.method),
            (TARGET_URI, request.target_uri),
            (ORIGIN, rim_origin),
        ]
    };
    let signature_params = signature_params(
        components,
        &[
            ("created", params[0].clone()),
            ("expires", params[1].clone()),
            ("nonce", params[2].clone()),
            ("keyid", params[3].clone()),
            ("alg", params[4].clone()),
            ("tag", params[5].clone()),
        ],
    )
    .ok_or(SignatureRefusal::Parameters)?;
    Ok(PendingRequest {
        nonce: nonce.to_string(),
        created,
        expires,
        rim_origin: rim_origin.to_string(),
        content_digest: if has_body {
            content_digest.map(str::to_string)
        } else {
            None
        },
        alg: alg.to_string(),
        base: signature_base(&lines, &signature_params),
        signature: parsed.signature,
        key_id,
    })
}

/// The `nonce` parameter of the profile's signature, read without checking anything else, so
/// a refusal can still echo it in `Rim-Request-Nonce`.
pub fn request_nonce(signature_input: &str) -> Option<String> {
    let inputs: Dictionary = Parser::new(signature_input).parse().ok()?;
    let Some(ListEntry::InnerList(list)) = inputs.get(SIGNATURE_LABEL) else {
        return None;
    };
    list.params
        .get("nonce")?
        .as_string()
        .map(|s| s.as_str().to_string())
}

/// The request a response answers, as both sides know it.
#[derive(Debug, Clone, Copy)]
pub struct RequestBinding<'a> {
    pub method: &'a str,
    pub target_uri: &'a str,
    /// The request's `Content-Digest`, when it had a body.
    pub content_digest: Option<&'a str>,
    /// The request's nonce, echoed in `Rim-Request-Nonce`.
    pub nonce: &'a str,
}

/// A response a server is about to send.
#[derive(Debug, Clone)]
pub struct OutgoingResponse<'a> {
    pub status: u16,
    pub body: &'a [u8],
    pub content_type: &'a str,
    pub request: RequestBinding<'a>,
    pub created: i64,
    pub key_id: &'a KeyId,
}

fn response_lines<'a>(
    status: &'a str,
    digest: &'a str,
    content_type: &'a str,
    request: &RequestBinding<'a>,
) -> (&'static [Component], Vec<(Component, &'a str)>) {
    let mut lines = vec![
        (STATUS, status),
        (CONTENT_DIGEST, digest),
        (CONTENT_TYPE, content_type),
        (REQUEST_NONCE, request.nonce),
        (REQ_METHOD, request.method),
        (REQ_TARGET_URI, request.target_uri),
    ];
    match request.content_digest {
        Some(req_digest) => {
            lines.push((REQ_CONTENT_DIGEST, req_digest));
            (RESPONSE_TO_BODY, lines)
        }
        None => (RESPONSE_TO_NO_BODY, lines),
    }
}

/// Sign a response. The caller sends the returned headers, `Content-Type` as given and
/// `Rim-Request-Nonce` with the request's nonce.
pub fn sign_response(
    signer: &ServerSigner,
    response: &OutgoingResponse<'_>,
) -> Option<SignatureHeaders> {
    let digest = content_digest(response.body);
    let status = response.status.to_string();
    let (components, lines) =
        response_lines(&status, &digest, response.content_type, &response.request);
    let params = signature_params(
        components,
        &[
            ("created", Param::Int(response.created)),
            ("keyid", Param::Str(response.key_id.to_string())),
            ("alg", Param::Str(signer.algorithm().name().to_string())),
            ("tag", Param::Str(SIGNATURE_TAG.to_string())),
        ],
    )?;
    let base = signature_base(&lines, &params);
    Some(SignatureHeaders {
        signature: signature_field(&signer.sign_bytes(base.as_bytes())),
        content_digest: Some(digest),
        signature_input: format!("{SIGNATURE_LABEL}={params}"),
    })
}

/// A response as the requester sees it.
#[derive(Debug, Clone, Default)]
pub struct IncomingResponse<'a> {
    pub status: u16,
    pub content_digest: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub rim_request_nonce: Option<&'a str>,
    pub signature_input: Option<&'a str>,
    pub signature: Option<&'a str>,
}

/// A response whose signature fits the profile and answers the expected request, ready to be
/// checked against the responder's key.
#[derive(Debug, Clone)]
pub struct PendingResponse {
    pub key_id: KeyId,
    pub created: i64,
    content_digest: String,
    alg: String,
    base: String,
    signature: Vec<u8>,
}

impl PendingResponse {
    pub fn check_digest(&self, body: &[u8]) -> Result<(), SignatureRefusal> {
        if digest_matches(&self.content_digest, body) {
            Ok(())
        } else {
            Err(SignatureRefusal::Digest)
        }
    }

    pub fn verify(&self, key: &PublicKey) -> Result<(), SignatureRefusal> {
        verify_base(key, &self.alg, &self.base, &self.signature)
    }
}

/// Check a response's signature against the profile and the request it answers: the
/// components (with the request's digest exactly when it had a body), the parameters, the
/// echoed nonce, and `created` within 300 s of `now`.
pub fn parse_response(
    response: &IncomingResponse<'_>,
    request: &RequestBinding<'_>,
    now: i64,
) -> Result<PendingResponse, SignatureRefusal> {
    let expected = if request.content_digest.is_some() {
        RESPONSE_TO_BODY
    } else {
        RESPONSE_TO_NO_BODY
    };
    let parsed = parse_signature(response.signature_input, response.signature, expected)?;
    let params = read_params(&parsed.params, RESPONSE_PARAMS)?;
    let created = int(&params[0]);
    let (keyid, alg, tag) = (text(&params[1]), text(&params[2]), text(&params[3]));
    if tag != SIGNATURE_TAG {
        return Err(SignatureRefusal::Parameters);
    }
    if (created - now).abs() > MAX_WINDOW_SECS {
        return Err(SignatureRefusal::Clock);
    }
    let key_id = KeyId::parse(keyid).ok_or(SignatureRefusal::KeyId)?;
    if response.rim_request_nonce.map(str::trim) != Some(request.nonce) {
        return Err(SignatureRefusal::RequestNonce);
    }
    let (Some(digest), Some(content_type)) = (
        response.content_digest.map(str::trim),
        response.content_type.map(str::trim),
    ) else {
        return Err(SignatureRefusal::MissingHeader);
    };
    let status = response.status.to_string();
    let (components, lines) = response_lines(&status, digest, content_type, request);
    let signature_params = signature_params(
        components,
        &[
            ("created", params[0].clone()),
            ("keyid", params[1].clone()),
            ("alg", params[2].clone()),
            ("tag", params[3].clone()),
        ],
    )
    .ok_or(SignatureRefusal::Parameters)?;
    Ok(PendingResponse {
        key_id,
        created,
        content_digest: digest.to_string(),
        alg: alg.to_string(),
        base: signature_base(&lines, &signature_params),
        signature: parsed.signature,
    })
}

/// Padded standard base64 of `bytes`.
pub fn b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key_log::{server_id_for_inception, sign_inception};

    const NOW: i64 = 1_791_504_000;

    struct Fixture {
        signer: ServerSigner,
        key_id: KeyId,
        nonce: String,
    }

    fn fixture() -> Fixture {
        let signer = ServerSigner::from_seed([9; 32]);
        let inception = sign_inception(
            &signer,
            &ServerSigner::from_seed([10; 32]),
            chrono::Utc::now(),
        );
        Fixture {
            key_id: KeyId {
                server_id: server_id_for_inception(&inception.payload),
                seq: 0,
            },
            signer,
            nonce: URL_SAFE_NO_PAD.encode([7; NONCE_LEN]),
        }
    }

    fn post<'a>(f: &'a Fixture, body: &'a [u8]) -> OutgoingRequest<'a> {
        OutgoingRequest {
            method: "POST",
            target_uri: "https://host.example/s2s/v1/x",
            body: Some(body),
            content_type: "application/json",
            rim_origin: "https://relay.example",
            created: NOW,
            expires: NOW + 300,
            nonce: &f.nonce,
            key_id: &f.key_id,
        }
    }

    fn incoming<'a>(out: &'a OutgoingRequest<'a>, h: &'a SignatureHeaders) -> IncomingRequest<'a> {
        IncomingRequest {
            method: out.method,
            target_uri: out.target_uri,
            content_digest: h.content_digest.as_deref(),
            content_type: Some(out.content_type),
            rim_origin: Some(out.rim_origin),
            signature_input: Some(&h.signature_input),
            signature: Some(&h.signature),
        }
    }

    fn check(
        req: &IncomingRequest<'_>,
        body: &[u8],
        key: &PublicKey,
        now: i64,
    ) -> Result<PendingRequest, SignatureRefusal> {
        let pending = parse_request(req, now)?;
        pending.check_digest(body)?;
        pending.verify(key)?;
        Ok(pending)
    }

    #[test]
    fn signed_post_verifies() {
        let f = fixture();
        let out = post(&f, b"{}");
        let h = sign_request(&f.signer, &out).expect("sign");
        let pending =
            check(&incoming(&out, &h), b"{}", &f.signer.public_key(), NOW).expect("verify");
        assert_eq!(pending.key_id, f.key_id);
        assert_eq!(pending.nonce, f.nonce);
        assert_eq!(pending.rim_origin, "https://relay.example");
    }

    #[test]
    fn signed_get_verifies_without_digest() {
        let f = fixture();
        let mut out = post(&f, b"");
        out.method = "GET";
        out.body = None;
        let h = sign_request(&f.signer, &out).expect("sign");
        assert!(h.content_digest.is_none());
        assert!(
            h.signature_input
                .starts_with(r#"sig1=("@method" "@target-uri" "rim-origin");"#)
        );
        check(&incoming(&out, &h), b"", &f.signer.public_key(), NOW).expect("verify");
        // A GET with a body is refused.
        let pending = parse_request(&incoming(&out, &h), NOW).expect("parse");
        assert_eq!(pending.check_digest(b"x"), Err(SignatureRefusal::Digest));
    }

    #[test]
    fn tampered_body_target_or_origin_fails() {
        let f = fixture();
        let out = post(&f, b"{\"a\":1}");
        let h = sign_request(&f.signer, &out).expect("sign");
        let key = f.signer.public_key();
        assert_eq!(
            check(&incoming(&out, &h), b"{\"a\":2}", &key, NOW).unwrap_err(),
            SignatureRefusal::Digest
        );
        let mut req = incoming(&out, &h);
        req.target_uri = "https://host.example/s2s/v1/y";
        assert_eq!(
            check(&req, b"{\"a\":1}", &key, NOW).unwrap_err(),
            SignatureRefusal::Verify
        );
        let mut req = incoming(&out, &h);
        req.rim_origin = Some("https://evil.example");
        assert_eq!(
            check(&req, b"{\"a\":1}", &key, NOW).unwrap_err(),
            SignatureRefusal::Verify
        );
        let other = ServerSigner::from_seed([11; 32]).public_key();
        assert_eq!(
            check(&incoming(&out, &h), b"{\"a\":1}", &other, NOW).unwrap_err(),
            SignatureRefusal::Verify
        );
    }

    #[test]
    fn clock_window_is_enforced() {
        let f = fixture();
        let key = f.signer.public_key();
        let sign_at = |created: i64, expires: i64| {
            let mut out = post(&f, b"{}");
            out.created = created;
            out.expires = expires;
            let h = sign_request(&f.signer, &out).expect("sign");
            (out, h)
        };
        // created too far ahead
        let (out, h) = sign_at(NOW + 301, NOW + 400);
        assert_eq!(
            check(&incoming(&out, &h), b"{}", &key, NOW).unwrap_err(),
            SignatureRefusal::Clock
        );
        // created at the limit
        let (out, h) = sign_at(NOW + 300, NOW + 400);
        assert!(check(&incoming(&out, &h), b"{}", &key, NOW).is_ok());
        // expired
        let (out, h) = sign_at(NOW - 600, NOW - 300);
        assert_eq!(
            check(&incoming(&out, &h), b"{}", &key, NOW).unwrap_err(),
            SignatureRefusal::Clock
        );
        let (out, h) = sign_at(NOW - 300, NOW);
        assert_eq!(
            check(&incoming(&out, &h), b"{}", &key, NOW).unwrap_err(),
            SignatureRefusal::Clock
        );
        // window too long
        let (out, h) = sign_at(NOW, NOW + 301);
        assert_eq!(
            check(&incoming(&out, &h), b"{}", &key, NOW).unwrap_err(),
            SignatureRefusal::Clock
        );
    }

    /// Re-sign a request's base after editing its `Signature-Input`, so the refusal is the
    /// profile check and not the signature.
    fn with_input(f: &Fixture, input: &str) -> (String, String) {
        let sig = signature_field(&f.signer.sign_bytes(b"irrelevant"));
        (input.to_string(), sig)
    }

    #[test]
    fn components_must_be_exactly_the_profiles() {
        let f = fixture();
        let out = post(&f, b"{}");
        let h = sign_request(&f.signer, &out).expect("sign");
        let params = h
            .signature_input
            .split_once(')')
            .expect("list")
            .1
            .to_string();
        for list in [
            r#"("@method" "@target-uri" "content-digest" "rim-origin")"#,
            r#"("@target-uri" "@method" "content-digest" "content-type" "rim-origin")"#,
            r#"("@method" "@target-uri" "content-digest" "content-type" "rim-origin" "date")"#,
            r#"("@method";req "@target-uri" "content-digest" "content-type" "rim-origin")"#,
        ] {
            let (input, sig) = with_input(&f, &format!("sig1={list}{params}"));
            let mut req = incoming(&out, &h);
            req.signature_input = Some(&input);
            req.signature = Some(&sig);
            assert_eq!(
                parse_request(&req, NOW).unwrap_err(),
                SignatureRefusal::Components,
                "{list}"
            );
        }
        // A GET signed as a POST.
        let mut req = incoming(&out, &h);
        req.method = "GET";
        assert_eq!(
            parse_request(&req, NOW).unwrap_err(),
            SignatureRefusal::Components
        );
        req.method = "PUT";
        assert_eq!(
            parse_request(&req, NOW).unwrap_err(),
            SignatureRefusal::Components
        );
    }

    #[test]
    fn parameters_must_be_exactly_the_profiles() {
        let f = fixture();
        let out = post(&f, b"{}");
        let h = sign_request(&f.signer, &out).expect("sign");
        let (list, _) = h.signature_input.split_once(';').expect("params");
        let keyid = f.key_id.to_string();
        let nonce = &f.nonce;
        let cases = [
            // reordered
            format!(
                r#";expires={};created={NOW};nonce="{nonce}";keyid="{keyid}";alg="ed25519";tag="rim-s2s-v1""#,
                NOW + 300
            ),
            // wrong alg name
            format!(
                r#";created={NOW};expires={};nonce="{nonce}";keyid="{keyid}";alg="rsa-pss-sha512";tag="rim-s2s-v1""#,
                NOW + 300
            ),
            // wrong tag
            format!(
                r#";created={NOW};expires={};nonce="{nonce}";keyid="{keyid}";alg="ed25519";tag="other""#,
                NOW + 300
            ),
            // missing nonce
            format!(
                r#";created={NOW};expires={};keyid="{keyid}";alg="ed25519";tag="rim-s2s-v1""#,
                NOW + 300
            ),
            // short nonce
            format!(
                r#";created={NOW};expires={};nonce="AAAA";keyid="{keyid}";alg="ed25519";tag="rim-s2s-v1""#,
                NOW + 300
            ),
            // extra parameter
            format!(
                r#";created={NOW};expires={};nonce="{nonce}";keyid="{keyid}";alg="ed25519";tag="rim-s2s-v1";x=1"#,
                NOW + 300
            ),
            // bad keyid
            format!(
                r#";created={NOW};expires={};nonce="{nonce}";keyid="rims1_x#0";alg="ed25519";tag="rim-s2s-v1""#,
                NOW + 300
            ),
        ];
        for (i, params) in cases.iter().enumerate() {
            let (input, sig) = with_input(&f, &format!("{list}{params}"));
            let mut req = incoming(&out, &h);
            req.signature_input = Some(&input);
            req.signature = Some(&sig);
            let pending = parse_request(&req, NOW);
            match i {
                1 => {
                    // Well-formed but not the key's algorithm.
                    let pending = pending.expect("parse");
                    assert_eq!(
                        pending.verify(&f.signer.public_key()).unwrap_err(),
                        SignatureRefusal::Algorithm
                    );
                }
                6 => assert_eq!(pending.unwrap_err(), SignatureRefusal::KeyId),
                _ => assert_eq!(
                    pending.unwrap_err(),
                    SignatureRefusal::Parameters,
                    "{params}"
                ),
            }
        }
    }

    #[test]
    fn only_another_label_is_refused_and_other_labels_are_ignored() {
        let f = fixture();
        let out = post(&f, b"{}");
        let h = sign_request(&f.signer, &out).expect("sign");
        let other_input = h.signature_input.replacen("sig1=", "sig2=", 1);
        let other_sig = h.signature.replacen("sig1=", "sig2=", 1);
        let mut req = incoming(&out, &h);
        req.signature_input = Some(&other_input);
        req.signature = Some(&other_sig);
        assert_eq!(
            parse_request(&req, NOW).unwrap_err(),
            SignatureRefusal::MissingLabel
        );

        let both_input = format!("{other_input}, {}", h.signature_input);
        let both_sig = format!("{other_sig}, {}", h.signature);
        req.signature_input = Some(&both_input);
        req.signature = Some(&both_sig);
        check(&req, b"{}", &f.signer.public_key(), NOW).expect("sig1 among others");

        let mut req = incoming(&out, &h);
        req.signature = None;
        assert_eq!(
            parse_request(&req, NOW).unwrap_err(),
            SignatureRefusal::MissingHeader
        );
        let mut req = incoming(&out, &h);
        req.rim_origin = None;
        assert_eq!(
            parse_request(&req, NOW).unwrap_err(),
            SignatureRefusal::MissingHeader
        );
    }

    #[test]
    fn content_digest_ignores_other_algorithms_and_needs_sha256() {
        let digest = content_digest(b"abc");
        assert!(digest_matches(&digest, b"abc"));
        assert!(digest_matches(&format!("sha-512=:AAAA:, {digest}"), b"abc"));
        assert!(!digest_matches("sha-512=:AAAA:", b"abc"));
        assert!(!digest_matches(&digest, b"abd"));
        assert!(!digest_matches("garbage(", b"abc"));
    }

    #[test]
    fn response_is_bound_to_the_request_nonce() {
        let f = fixture();
        let out = post(&f, b"{}");
        let h = sign_request(&f.signer, &out).expect("sign");
        let binding = RequestBinding {
            method: "POST",
            target_uri: out.target_uri,
            content_digest: h.content_digest.as_deref(),
            nonce: &f.nonce,
        };
        let body = br#"{"ok":true}"#;
        let resp = OutgoingResponse {
            status: 200,
            body,
            content_type: "application/json",
            request: binding,
            created: NOW,
            key_id: &f.key_id,
        };
        let rh = sign_response(&f.signer, &resp).expect("sign");
        let incoming = IncomingResponse {
            status: 200,
            content_digest: rh.content_digest.as_deref(),
            content_type: Some("application/json"),
            rim_request_nonce: Some(&f.nonce),
            signature_input: Some(&rh.signature_input),
            signature: Some(&rh.signature),
        };
        let pending = parse_response(&incoming, &binding, NOW).expect("parse");
        pending.check_digest(body).expect("digest");
        pending.verify(&f.signer.public_key()).expect("verify");

        // Another request's nonce.
        let other_nonce = URL_SAFE_NO_PAD.encode([8; NONCE_LEN]);
        let other = RequestBinding {
            nonce: &other_nonce,
            ..binding
        };
        assert_eq!(
            parse_response(&incoming, &other, NOW).unwrap_err(),
            SignatureRefusal::RequestNonce
        );
        // The echoed nonce rewritten to match another request does not verify.
        let forged = IncomingResponse {
            rim_request_nonce: Some(&other_nonce),
            ..incoming.clone()
        };
        let pending = parse_response(&forged, &other, NOW).expect("parse");
        assert_eq!(
            pending.verify(&f.signer.public_key()).unwrap_err(),
            SignatureRefusal::Verify
        );
        // Another status.
        let changed = IncomingResponse {
            status: 500,
            ..incoming.clone()
        };
        let pending = parse_response(&changed, &binding, NOW).expect("parse");
        assert_eq!(
            pending.verify(&f.signer.public_key()).unwrap_err(),
            SignatureRefusal::Verify
        );
        // A response to a GET does not cover the request digest.
        let get = RequestBinding {
            method: "GET",
            content_digest: None,
            ..binding
        };
        assert_eq!(
            parse_response(&incoming, &get, NOW).unwrap_err(),
            SignatureRefusal::Components
        );
        // Too old.
        assert_eq!(
            parse_response(&incoming, &binding, NOW + 301).unwrap_err(),
            SignatureRefusal::Clock
        );
    }

    #[test]
    fn request_nonce_is_read_leniently() {
        let f = fixture();
        let out = post(&f, b"{}");
        let h = sign_request(&f.signer, &out).expect("sign");
        assert_eq!(request_nonce(&h.signature_input), Some(f.nonce.clone()));
        assert_eq!(
            request_nonce(r#"sig1=();nonce="x";extra=1"#),
            Some("x".into())
        );
        assert_eq!(request_nonce("sig2=();nonce=\"x\""), None);
        assert_eq!(request_nonce("garbage("), None);
    }

    #[test]
    fn keyid_parses_strictly() {
        let f = fixture();
        let id = &f.key_id.server_id;
        assert_eq!(KeyId::parse(&format!("{id}#12")).map(|k| k.seq), Some(12));
        for bad in [
            format!("{id}#"),
            format!("{id}#01"),
            format!("{id}#-1"),
            id.to_string(),
            "x#0".to_string(),
        ] {
            assert_eq!(KeyId::parse(&bad), None, "{bad}");
        }
    }
}
