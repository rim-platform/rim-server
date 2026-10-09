//! The sealed channel through which a signed-in device certifies a new device's key.
//!
//! The old device shows a [`LinkCode`] (`rim-link:1:<linkId>:<secret>:<endpoint>`); both
//! devices derive one AES-256-GCM key per direction from its secret ([`Channel`]); the new
//! device seals an [`Offer`] and the old device answers with an [`Answer`] carrying the device
//! certificate. Both screens show the offer's [`Fingerprint`] for the person to compare.
//!
//! The server only relays the sealed bytes and never calls this module; clients and tests do.

use std::fmt;

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hkdf::Hkdf;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::device::{DeviceCert, MAX_DEVICE_NAME_CHARS, verify_device_cert};
use crate::encoding::{PublicKey, b64};
use crate::error::{IdentityError, IdentityResult};
use crate::key_log::KeyState;
use crate::statement::{SignedStatement, check_label};

/// The version of everything in this module: the code, the offer and answer `"v"`, the HKDF
/// `info` strings and the AAD.
pub const CODE_VERSION: u32 = 1;

/// Written lowercase; read ASCII case-insensitively.
const SCHEME: &str = "rim-link:";

pub const LINK_ID_LEN: usize = 16;
pub const SECRET_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
/// The AES-GCM tag, appended to the ciphertext (what WebCrypto's `encrypt` returns).
pub const TAG_LEN: usize = 16;

/// Longest endpoint in a code, in bytes.
pub const MAX_ENDPOINT_LEN: usize = 256;

/// Longest code a reader accepts once whitespace is removed: the 78 fixed characters
/// (`rim-link:1:`, a 22-character link id, `:`, a 43-character secret, `:`) plus the endpoint.
pub const MAX_CODE_LEN: usize = 78 + MAX_ENDPOINT_LEN;

const AAD_PREFIX: &[u8] = b"rim.device-link.v1\0";
const FINGERPRINT_DOMAIN: &[u8] = b"rim.device-link.v1 fingerprint\0";
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Why a code, sealed message or plaintext was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeviceLinkError {
    #[error("this is not a device link code")]
    NotALinkCode,

    #[error("a device link code is at most {MAX_CODE_LEN} characters")]
    CodeTooLong,

    #[error("this code was made by a newer app; update this device")]
    UnsupportedVersion,

    #[error("{0} is not in its canonical encoding")]
    BadEncoding(&'static str),

    #[error(
        "the endpoint must start with http:// or https://, be 1 to {MAX_ENDPOINT_LEN} printable \
         ASCII characters and contain no `?`, `#` or `@`"
    )]
    BadEndpoint,

    #[error("the plaintext exceeds {max} bytes")]
    TooLarge { max: usize },

    #[error("the message could not be sealed")]
    SealFailed,

    /// The message failed to open, or opened to a plaintext that breaks the offer or answer
    /// rules. The two are deliberately one error: either way, someone other than the intended
    /// device wrote it.
    #[error("the sealed message does not open under this link")]
    Unreadable,
}

pub type DeviceLinkResult<T> = Result<T, DeviceLinkError>;

fn decode_fixed<const N: usize>(s: &str, what: &'static str) -> DeviceLinkResult<[u8; N]> {
    // The engine refuses padding and non-zero trailing bits, so each value has one spelling.
    let bytes = URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| DeviceLinkError::BadEncoding(what))?;
    bytes
        .try_into()
        .map_err(|_| DeviceLinkError::BadEncoding(what))
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

/// The 16-byte link id the server makes. A bearer capability: whoever holds it can post an
/// offer, read the answer and cancel the link.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct LinkId([u8; LINK_ID_LEN]);

impl LinkId {
    /// A link id from its raw bytes.
    pub fn from_bytes(bytes: [u8; LINK_ID_LEN]) -> Self {
        Self(bytes)
    }

    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8; LINK_ID_LEN] {
        &self.0
    }

    /// Strict unpadded base64url of exactly 16 bytes (22 characters).
    pub fn parse(s: &str) -> DeviceLinkResult<Self> {
        decode_fixed(s, "link id").map(Self)
    }
}

impl fmt::Display for LinkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&b64(&self.0))
    }
}

impl fmt::Debug for LinkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LinkId({self})")
    }
}

/// The 32-byte secret the old device makes. It never reaches the server.
#[derive(Clone, PartialEq, Eq)]
pub struct LinkSecret([u8; SECRET_LEN]);

impl LinkSecret {
    /// A fresh secret from the OS random generator.
    pub fn generate() -> Self {
        Self(random_bytes())
    }

    /// A secret from its raw bytes.
    pub fn from_bytes(bytes: [u8; SECRET_LEN]) -> Self {
        Self(bytes)
    }

    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8; SECRET_LEN] {
        &self.0
    }

    /// Strict unpadded base64url of exactly 32 bytes (43 characters).
    pub fn parse(s: &str) -> DeviceLinkResult<Self> {
        decode_fixed(s, "secret").map(Self)
    }

    /// The unpadded base64url text written into the code.
    pub fn encode(&self) -> String {
        b64(&self.0)
    }
}

impl fmt::Debug for LinkSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LinkSecret(..)")
    }
}

fn check_endpoint(endpoint: &str) -> DeviceLinkResult<()> {
    let ok = (endpoint.starts_with("http://") || endpoint.starts_with("https://"))
        && endpoint.len() <= MAX_ENDPOINT_LEN
        && endpoint
            .bytes()
            .all(|b| (0x21..=0x7e).contains(&b) && !matches!(b, b'?' | b'#' | b'@'));
    if ok {
        Ok(())
    } else {
        Err(DeviceLinkError::BadEndpoint)
    }
}

/// What the old device shows as a QR code and as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkCode {
    pub link_id: LinkId,
    pub secret: LinkSecret,
    /// The old device's GraphQL endpoint, without a trailing `/`.
    pub endpoint: String,
}

impl LinkCode {
    /// A code for `endpoint` as configured; trailing `/` characters are removed.
    pub fn new(link_id: LinkId, secret: LinkSecret, endpoint: &str) -> DeviceLinkResult<Self> {
        let endpoint = endpoint.trim_end_matches('/');
        check_endpoint(endpoint)?;
        Ok(Self {
            link_id,
            secret,
            endpoint: endpoint.to_string(),
        })
    }

    /// Read a code as pasted: ASCII whitespace (tab, LF, FF, CR, space) anywhere is ignored,
    /// the scheme is compared case-insensitively, and the endpoint keeps its own colons.
    pub fn parse(text: &str) -> DeviceLinkResult<Self> {
        let text: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        if text.len() > MAX_CODE_LEN {
            return Err(DeviceLinkError::CodeTooLong);
        }
        let rest = text
            .get(..SCHEME.len())
            .filter(|scheme| scheme.eq_ignore_ascii_case(SCHEME))
            .and_then(|_| text.get(SCHEME.len()..))
            .ok_or(DeviceLinkError::NotALinkCode)?;
        let mut parts = rest.splitn(4, ':');
        let (Some(version), Some(link_id), Some(secret), Some(endpoint)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(DeviceLinkError::NotALinkCode);
        };
        // A version is a decimal number above 0, spelled without leading zeros.
        let version = version
            .parse::<u32>()
            .ok()
            .filter(|v| *v > 0 && v.to_string() == version)
            .ok_or(DeviceLinkError::NotALinkCode)?;
        if version != CODE_VERSION {
            return Err(DeviceLinkError::UnsupportedVersion);
        }
        let link_id = LinkId::parse(link_id)?;
        let secret = LinkSecret::parse(secret)?;
        check_endpoint(endpoint)?;
        Ok(Self {
            link_id,
            secret,
            endpoint: endpoint.to_string(),
        })
    }

    /// The channel both devices seal and open with.
    pub fn channel(&self) -> Channel {
        Channel::new(&self.link_id, &self.secret)
    }
}

impl fmt::Display for LinkCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{SCHEME}{CODE_VERSION}:{}:{}:{}",
            self.link_id,
            self.secret.encode(),
            self.endpoint
        )
    }
}

/// Which way a sealed message travels. Each direction has its own key and AAD, so an offer
/// never opens as an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// New device to old device.
    Offer,
    /// Old device to new device.
    Answer,
}

impl Direction {
    fn name(self) -> &'static str {
        match self {
            Self::Offer => "offer",
            Self::Answer => "answer",
        }
    }

    fn info(self) -> &'static [u8] {
        match self {
            Self::Offer => b"rim.device-link.v1 offer",
            Self::Answer => b"rim.device-link.v1 answer",
        }
    }

    /// Largest plaintext sealed in this direction, sized for hybrid post-quantum keys.
    pub fn max_plaintext_len(self) -> usize {
        match self {
            Self::Offer => 16 * 1024,
            Self::Answer => 64 * 1024,
        }
    }

    /// Largest `ciphertext` (plaintext plus tag) in this direction; servers refuse more.
    pub fn max_ciphertext_len(self) -> usize {
        self.max_plaintext_len() + TAG_LEN
    }
}

/// `K = HKDF-SHA256(ikm = secret, salt = link id, info = "rim.device-link.v1 <direction>")`.
fn derive_key(link_id: &LinkId, secret: &LinkSecret, direction: Direction) -> [u8; 32] {
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(link_id.as_bytes()), secret.as_bytes())
        .expand(direction.info(), &mut key)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    key
}

/// `rim.device-link.v1\0<direction>\0<link id text>`.
fn aad(link_id: &LinkId, direction: Direction) -> Vec<u8> {
    let link_id = link_id.to_string();
    let mut aad = Vec::with_capacity(AAD_PREFIX.len() + 7 + link_id.len());
    aad.extend_from_slice(AAD_PREFIX);
    aad.extend_from_slice(direction.name().as_bytes());
    aad.push(0);
    aad.extend_from_slice(link_id.as_bytes());
    aad
}

/// A nonce and AES-GCM output with the tag appended, as `sendDeviceLinkOffer`,
/// `sendDeviceLinkAnswer` and the queries carry them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedMessage {
    pub nonce: [u8; NONCE_LEN],
    pub ciphertext: Vec<u8>,
}

impl SealedMessage {
    /// Read the wire form: both fields strict unpadded base64url, the nonce exactly 12 bytes.
    pub fn from_text(nonce: &str, ciphertext: &str) -> DeviceLinkResult<Self> {
        Ok(Self {
            nonce: decode_fixed(nonce, "nonce")?,
            ciphertext: URL_SAFE_NO_PAD
                .decode(ciphertext)
                .map_err(|_| DeviceLinkError::BadEncoding("ciphertext"))?,
        })
    }

    /// The nonce as unpadded base64url.
    pub fn nonce_text(&self) -> String {
        b64(&self.nonce)
    }

    /// The ciphertext as unpadded base64url.
    pub fn ciphertext_text(&self) -> String {
        b64(&self.ciphertext)
    }
}

/// The 8-character code both devices show for an offer, so the person can tell their own
/// device's offer from one posted by someone else who saw the link code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint([u8; 5]);

impl Fingerprint {
    /// `base32(first 5 bytes of SHA-256("rim.device-link.v1 fingerprint\0" || plaintext))`,
    /// over the offer bytes as sealed (new device) or as opened (old device).
    pub fn of_offer(plaintext: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(FINGERPRINT_DOMAIN);
        hasher.update(plaintext);
        let digest = hasher.finalize();
        let mut bytes = [0u8; 5];
        bytes.copy_from_slice(&digest[..5]);
        Self(bytes)
    }

    /// RFC 4648 base32, uppercase, no padding: 8 characters.
    pub fn code(&self) -> String {
        let bits = self
            .0
            .iter()
            .fold(0u64, |acc, &b| (acc << 8) | u64::from(b));
        (0..8)
            .rev()
            .map(|i| char::from(BASE32_ALPHABET[((bits >> (i * 5)) & 0x1f) as usize]))
            .collect()
    }
}

/// Shown as `XXXX-XXXX`.
impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = self.code();
        write!(f, "{}-{}", &code[..4], &code[4..])
    }
}

/// Where the new device runs. Self-reported, so a display hint only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Desktop,
    Web,
}

impl Platform {
    /// The value written in the offer's `platform`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Web => "web",
        }
    }
}

/// Lowest key-identity statement version an offer may name.
const MIN_OFFER_STATEMENT_VERSION: u32 = 2;

fn check_code_header(v: u32, kind: &str, expected: &'static str) -> IdentityResult<()> {
    if v != CODE_VERSION {
        return Err(IdentityError::UnsupportedVersion(v));
    }
    if kind != expected {
        return Err(IdentityError::WrongType { expected });
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawOffer {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    device_id: String,
    device_key: String,
    name: String,
    platform: Platform,
    statement_version: u32,
}

/// What the new device seals: the key it wants certified and how to call it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// Chosen by the new device (UUID v7 recommended); written lowercase with hyphens.
    pub device_id: Uuid,
    pub device_key: PublicKey,
    /// The certificate name rules apply; trimmed when read.
    pub name: String,
    pub platform: Platform,
    /// The highest key-identity statement version the new device reads; at least 2.
    pub statement_version: u32,
}

impl Offer {
    /// The plaintext: fields in a fixed order, no spaces.
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"v":{CODE_VERSION},"type":"link-offer","deviceId":"{}","deviceKey":"{}","name":{},"platform":"{}","statementVersion":{}}}"#,
            self.device_id.hyphenated(),
            self.device_key,
            serde_json::Value::from(self.name.as_str()),
            self.platform.as_str(),
            self.statement_version,
        )
    }

    /// Parse an opened plaintext. Unknown and duplicate fields, another `type` or `v`, a
    /// non-canonical device id, an untagged or weak key and a bad name are refused.
    pub fn parse(plaintext: &[u8]) -> IdentityResult<Self> {
        let raw: RawOffer = serde_json::from_slice(plaintext)
            .map_err(|e| IdentityError::Malformed(e.to_string()))?;
        check_code_header(raw.v, &raw.kind, "link-offer")?;
        let device_id = Uuid::parse_str(&raw.device_id)
            .ok()
            .filter(|id| id.hyphenated().to_string() == raw.device_id)
            .ok_or(IdentityError::BadEncoding("deviceId"))?;
        let device_key = PublicKey::parse(&raw.device_key, "deviceKey")?;
        let name = check_label(&raw.name, "name", MAX_DEVICE_NAME_CHARS)?;
        if raw.statement_version < MIN_OFFER_STATEMENT_VERSION {
            return Err(IdentityError::Invalid(format!(
                "statementVersion must be at least {MIN_OFFER_STATEMENT_VERSION}"
            )));
        }
        Ok(Self {
            device_id,
            device_key,
            name,
            platform: raw.platform,
            statement_version: raw.statement_version,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStatement {
    payload: String,
    signature: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAnswer {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    certificate: RawStatement,
}

/// What the old device seals back: the device certificate exactly as sent to `addDeviceKey`.
/// Nothing in it is secret, since anyone who saw the code can open it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub certificate: SignedStatement,
}

impl Answer {
    /// The plaintext: fields in a fixed order, no spaces.
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"v":{CODE_VERSION},"type":"link-answer","certificate":{{"payload":{},"signature":{}}}}}"#,
            serde_json::Value::from(self.certificate.payload.as_str()),
            serde_json::Value::from(self.certificate.signature.as_str()),
        )
    }

    /// Parse an opened plaintext, with the same strictness as [`Offer::parse`].
    pub fn parse(plaintext: &[u8]) -> IdentityResult<Self> {
        let raw: RawAnswer = serde_json::from_slice(plaintext)
            .map_err(|e| IdentityError::Malformed(e.to_string()))?;
        check_code_header(raw.v, &raw.kind, "link-answer")?;
        Ok(Self {
            certificate: SignedStatement {
                payload: raw.certificate.payload,
                signature: raw.certificate.signature,
            },
        })
    }

    /// Check the certificate as the new device must: a `device` statement signed by the
    /// identity key of `state` (the key log replayed up to the certificate's `keySeq`), naming
    /// exactly the offer's device id and key.
    pub fn verify(&self, offer: &Offer, state: &KeyState) -> IdentityResult<DeviceCert> {
        let cert = verify_device_cert(state, &self.certificate)?;
        if cert.device_id != offer.device_id || cert.device_key != offer.device_key {
            return Err(IdentityError::Invalid(
                "the certificate names another device than the offer".to_string(),
            ));
        }
        Ok(cert)
    }
}

/// An offer as the new device sealed it, with the fingerprint it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedOffer {
    pub sealed: SealedMessage,
    pub fingerprint: Fingerprint,
}

/// An offer as the old device opened it, with the fingerprint it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedOffer {
    pub offer: Offer,
    pub fingerprint: Fingerprint,
}

/// The two directional keys of one link.
///
/// Each side seals exactly one plaintext per link and resends the same sealed bytes on a
/// retry, so a random nonce never repeats under a key.
#[derive(Clone)]
pub struct Channel {
    link_id: LinkId,
    offer_key: [u8; 32],
    answer_key: [u8; 32],
}

impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Channel({})", self.link_id)
    }
}

impl Channel {
    /// Derive both keys of the link from its id and secret.
    pub fn new(link_id: &LinkId, secret: &LinkSecret) -> Self {
        Self {
            link_id: *link_id,
            offer_key: derive_key(link_id, secret, Direction::Offer),
            answer_key: derive_key(link_id, secret, Direction::Answer),
        }
    }

    fn cipher(&self, direction: Direction) -> Aes256Gcm {
        let key = match direction {
            Direction::Offer => &self.offer_key,
            Direction::Answer => &self.answer_key,
        };
        Aes256Gcm::new(key.into())
    }

    /// Seal `plaintext` with a fresh random nonce.
    pub fn seal(&self, direction: Direction, plaintext: &[u8]) -> DeviceLinkResult<SealedMessage> {
        self.seal_with_nonce(direction, random_bytes(), plaintext)
    }

    fn seal_with_nonce(
        &self,
        direction: Direction,
        nonce: [u8; NONCE_LEN],
        plaintext: &[u8],
    ) -> DeviceLinkResult<SealedMessage> {
        let max = direction.max_plaintext_len();
        if plaintext.len() > max {
            return Err(DeviceLinkError::TooLarge { max });
        }
        let ciphertext = self
            .cipher(direction)
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad(&self.link_id, direction),
                },
            )
            .map_err(|_| DeviceLinkError::SealFailed)?;
        Ok(SealedMessage { nonce, ciphertext })
    }

    /// Open a message sealed for this link in `direction`.
    pub fn open(&self, direction: Direction, sealed: &SealedMessage) -> DeviceLinkResult<Vec<u8>> {
        self.open_with_aad(direction, &aad(&self.link_id, direction), sealed)
    }

    fn open_with_aad(
        &self,
        direction: Direction,
        aad: &[u8],
        sealed: &SealedMessage,
    ) -> DeviceLinkResult<Vec<u8>> {
        let len = sealed.ciphertext.len();
        if len <= TAG_LEN || len > direction.max_ciphertext_len() {
            return Err(DeviceLinkError::Unreadable);
        }
        self.cipher(direction)
            .decrypt(
                Nonce::from_slice(&sealed.nonce),
                Payload {
                    msg: &sealed.ciphertext,
                    aad,
                },
            )
            .map_err(|_| DeviceLinkError::Unreadable)
    }

    /// Seal an offer (new device) and compute the fingerprint it shows.
    pub fn seal_offer(&self, offer: &Offer) -> DeviceLinkResult<SealedOffer> {
        let plaintext = offer.to_json();
        Ok(SealedOffer {
            sealed: self.seal(Direction::Offer, plaintext.as_bytes())?,
            fingerprint: Fingerprint::of_offer(plaintext.as_bytes()),
        })
    }

    /// Open and parse an offer (old device). A plaintext that breaks the offer rules is
    /// [`DeviceLinkError::Unreadable`], exactly like one that does not open.
    pub fn open_offer(&self, sealed: &SealedMessage) -> DeviceLinkResult<OpenedOffer> {
        let plaintext = self.open(Direction::Offer, sealed)?;
        let offer = Offer::parse(&plaintext).map_err(|_| DeviceLinkError::Unreadable)?;
        Ok(OpenedOffer {
            offer,
            fingerprint: Fingerprint::of_offer(&plaintext),
        })
    }

    /// Seal an answer (old device).
    pub fn seal_answer(&self, answer: &Answer) -> DeviceLinkResult<SealedMessage> {
        self.seal(Direction::Answer, answer.to_json().as_bytes())
    }

    /// Open and parse an answer (new device). A plaintext that breaks the answer rules is
    /// [`DeviceLinkError::Unreadable`], exactly like one that does not open.
    pub fn open_answer(&self, sealed: &SealedMessage) -> DeviceLinkResult<Answer> {
        let plaintext = self.open(Direction::Answer, sealed)?;
        Answer::parse(&plaintext).map_err(|_| DeviceLinkError::Unreadable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statement::StatementVersion::V2;
    use crate::testing::{TestIdentity, signing_key};

    const LINK_ID: &str = "AAECAwQFBgcICQoLDA0ODw";
    const SECRET: &str = "ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8";
    const ENDPOINT: &str = "http://localhost:8080/api";
    const CODE: &str = "rim-link:1:AAECAwQFBgcICQoLDA0ODw:ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8:http://localhost:8080/api";
    const K_OFFER: &str = "6dff67bf777c3cf073ebc0f8810dae7535d3bdb59c9178183441db3768212fba";
    const K_ANSWER: &str = "d6ec24d3f4380cb2ca50865f95a37f3e6b0ab9c2d74ee31d605768abe6bf56e1";
    const OFFER_AAD: &str = "72696d2e6465766963652d6c696e6b2e7631006f666665720041414543417751464267634943516f4c4441304f4477";
    const ANSWER_AAD: &str = "72696d2e6465766963652d6c696e6b2e763100616e737765720041414543417751464267634943516f4c4441304f4477";
    const OFFER_NONCE: &str = "QEFCQ0RFRkdISUpL";
    const ANSWER_NONCE: &str = "UFFSU1RVVldYWVpb";
    const FINGERPRINT: &str = "QSFX-VHMA";
    const FINGERPRINT_SHA256: &str =
        "848b7a9d80a01b87adceab1630b57a452359598c53e4326382d7a236e3ca8629";
    const OFFER_PLAINTEXT: &str = r#"{"v":1,"type":"link-offer","deviceId":"00000000-0000-0000-0000-000000000001","deviceKey":"ed25519:IIKL9cW9ystoSGMzbCAvtVmdpIvlWWYVdCFwcFvsqfc","name":"Laptop","platform":"desktop","statementVersion":2}"#;
    const OFFER_CIPHERTEXT: &str = "J8-HezTgXKj9OyT7HMBA1zjjB4ZTJEJkqjh4skQfSya0Sxe6uk9qWLSzUqUAQ7QrC4ZIEYlO_dsskNkfhQ_iEVXyyUkGmH2j1gqSNpQTetjmsgmQEXUbEsSs8ciJPmD_o_T8zR_R7k-rjthHmMQYHVfwslbojzQhR7wXQiUJfZtIDm5bfx9wYmPPjvmcA9RYQnV6_3GRGoFbsLE9fGt4mRlidnXDiOweblWqdi0dARTNJHJL7DDAMwmqGHiEIcAVTVu7B4SiHFPrTmVBclEOQUB5SxXIsO1wsQ";
    const OFFER_TAG: &str = "4e654172510e4140794b15c8b0ed70b1";
    const ANSWER_PLAINTEXT: &str = r#"{"v":1,"type":"link-answer","certificate":{"payload":"{\"v\":2,\"type\":\"device\",\"principalId\":\"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0\",\"deviceId\":\"00000000-0000-0000-0000-000000000001\",\"deviceKey\":\"ed25519:IIKL9cW9ystoSGMzbCAvtVmdpIvlWWYVdCFwcFvsqfc\",\"name\":\"Laptop\",\"keySeq\":0,\"issuedAt\":\"2026-10-09T00:00:00Z\"}","signature":"ed25519:0HjRzn6gQ2q-vCHSkdOPK2ykGGBUpW2fYdDuEdTzf6OxsWzjXeNNpRSVgzPxOEfbYzZ9o9DWqp4NxcTEDhOaDA"}}"#;
    const ANSWER_CIPHERTEXT: &str = "vkDrDIh3nkBRDHroeH_tmxnHXtoMzfo4grqtAiBG46z7xMsAlJDtvvExm51M7w7G7oglt9YXamu3_qjKYYDPA7b_xGvbPvWGkw98Hc91oL2qQvVYz0MSwIWu5R_FyFwN0SIVdph0xJxLBdXq3E3-iKFd4fdfLRdZvRi7qMMTY7wW8DMw2ItevB1tV2xfMF2_WpZNSUvPuJUF9l8VIdtiIHO1V94Sx-ombzLAvfndFHJfxKhsDwxPB9YSaKoaIvKPZrI_6Z0Mzwdn7RAyepsgjrOz4wDnoEWv5eFOO1zU4RpZvRwOmlhJ2NKwLsG87fEtw16kB3g2qMqvwGLWrQ4VuqygY3JucPE0Gu2H-dkxLEYrEuPMbGxMzD2ROIAlZFKljdgj8zGLrYbkLml1Jh-rJijDswN0ktaw-dCCqd0-Pbt4nXvBKGTV5ZuDJ1W-nIMYs9Py81klr5gBOypP8A3In4OFi7ePsu2FOnZE-LRYfaLeYv2PCPKCFaZnCjJS0asVa-UFXHhmz2sWxVwEi1OnENi11C4bweM6a_9ydUHi4w4QMBkZpfHCyondEdgCtxyq8xZXPsEP6_JX007Y9jm9gGQ8JExrsviYRMpeWFT1pVJ2HJJfbkrHrF1Fq7c";
    const ANSWER_TAG: &str = "54f5a552761c925f6e4ac7ac5d45abb7";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn bytes_from<const N: usize>(first: u8) -> [u8; N] {
        std::array::from_fn(|i| first + i as u8)
    }

    fn link_id() -> LinkId {
        LinkId::from_bytes(bytes_from(0x00))
    }

    fn secret() -> LinkSecret {
        LinkSecret::from_bytes(bytes_from(0x20))
    }

    fn code() -> LinkCode {
        LinkCode::new(link_id(), secret(), ENDPOINT).expect("code")
    }

    /// The known-answer device (seed 20, device id …0001, name `Laptop`) as a desktop offer.
    fn offer() -> Offer {
        Offer {
            device_id: Uuid::from_u128(1),
            device_key: PublicKey::from(&signing_key(20)),
            name: "Laptop".to_string(),
            platform: Platform::Desktop,
            statement_version: 2,
        }
    }

    /// The known-answer identity (seed 1, version 2), its head and its certificate for
    /// [`offer`]'s device.
    fn certified() -> (KeyState, Answer) {
        let id = TestIdentity::with_version(1, V2);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        let certificate = id.device_cert(&state, Uuid::from_u128(1), &signing_key(20), "Laptop");
        (state, Answer { certificate })
    }

    fn sealed_offer() -> SealedMessage {
        SealedMessage::from_text(OFFER_NONCE, OFFER_CIPHERTEXT).expect("sealed offer")
    }

    fn sealed_answer() -> SealedMessage {
        SealedMessage::from_text(ANSWER_NONCE, ANSWER_CIPHERTEXT).expect("sealed answer")
    }

    #[test]
    fn known_answer_code_keys_and_aad() {
        assert_eq!(link_id().to_string(), LINK_ID);
        assert_eq!(secret().encode(), SECRET);
        let code = code();
        assert_eq!(code.to_string(), CODE);
        assert_eq!(code.to_string().len(), 103);
        assert_eq!(LinkCode::parse(CODE), Ok(code));

        assert_eq!(
            hex(&derive_key(&link_id(), &secret(), Direction::Offer)),
            K_OFFER
        );
        assert_eq!(
            hex(&derive_key(&link_id(), &secret(), Direction::Answer)),
            K_ANSWER
        );
        assert_eq!(hex(&aad(&link_id(), Direction::Offer)), OFFER_AAD);
        assert_eq!(hex(&aad(&link_id(), Direction::Answer)), ANSWER_AAD);
        assert_eq!(b64(&bytes_from::<12>(0x40)), OFFER_NONCE);
        assert_eq!(b64(&bytes_from::<12>(0x50)), ANSWER_NONCE);
    }

    #[test]
    fn known_answer_offer() {
        let plaintext = offer().to_json();
        assert_eq!(plaintext, OFFER_PLAINTEXT);
        assert_eq!(plaintext.len(), 201);

        let channel = code().channel();
        let sealed = channel
            .seal_with_nonce(Direction::Offer, bytes_from(0x40), plaintext.as_bytes())
            .expect("seal");
        assert_eq!(sealed.nonce_text(), OFFER_NONCE);
        assert_eq!(sealed.ciphertext_text(), OFFER_CIPHERTEXT);
        assert_eq!(sealed.ciphertext.len(), 217);
        assert_eq!(hex(&sealed.ciphertext[201..]), OFFER_TAG);

        let mut hasher = Sha256::new();
        hasher.update(FINGERPRINT_DOMAIN);
        hasher.update(plaintext.as_bytes());
        assert_eq!(hex(&hasher.finalize()), FINGERPRINT_SHA256);
        assert_eq!(
            Fingerprint::of_offer(plaintext.as_bytes()).to_string(),
            FINGERPRINT
        );
        assert_eq!(
            Fingerprint::of_offer(plaintext.as_bytes()).code(),
            "QSFXVHMA"
        );

        let opened = channel.open_offer(&sealed_offer()).expect("open offer");
        assert_eq!(opened.offer, offer());
        assert_eq!(opened.fingerprint.to_string(), FINGERPRINT);
    }

    #[test]
    fn known_answer_answer() {
        let (state, answer) = certified();
        let plaintext = answer.to_json();
        assert_eq!(plaintext, ANSWER_PLAINTEXT);
        assert_eq!(plaintext.len(), 460);

        let channel = code().channel();
        let sealed = channel
            .seal_with_nonce(Direction::Answer, bytes_from(0x50), plaintext.as_bytes())
            .expect("seal");
        assert_eq!(sealed.nonce_text(), ANSWER_NONCE);
        assert_eq!(sealed.ciphertext_text(), ANSWER_CIPHERTEXT);
        assert_eq!(sealed.ciphertext.len(), 476);
        assert_eq!(hex(&sealed.ciphertext[460..]), ANSWER_TAG);

        let opened = channel.open_answer(&sealed_answer()).expect("open answer");
        assert_eq!(opened, answer);
        let cert = opened.verify(&offer(), &state).expect("verify");
        assert_eq!(cert.device_id, Uuid::from_u128(1));
    }

    #[test]
    fn seal_uses_a_fresh_nonce_and_round_trips() {
        let channel = code().channel();
        let sealed = channel.seal_offer(&offer()).expect("seal");
        let again = channel.seal_offer(&offer()).expect("seal");
        assert_ne!(sealed.sealed.nonce, again.sealed.nonce);
        assert_eq!(sealed.fingerprint, again.fingerprint);
        let opened = channel.open_offer(&sealed.sealed).expect("open");
        assert_eq!(opened.offer, offer());
        assert_eq!(opened.fingerprint, sealed.fingerprint);

        let (_, answer) = certified();
        let sealed = channel.seal_answer(&answer).expect("seal answer");
        assert_eq!(channel.open_answer(&sealed), Ok(answer));
    }

    #[test]
    fn an_offer_never_opens_as_an_answer_or_under_another_link() {
        let channel = code().channel();
        assert_eq!(
            channel.open(Direction::Answer, &sealed_offer()),
            Err(DeviceLinkError::Unreadable)
        );
        assert_eq!(
            channel.open_answer(&sealed_offer()),
            Err(DeviceLinkError::Unreadable)
        );
        assert_eq!(
            channel.open_offer(&sealed_answer()),
            Err(DeviceLinkError::Unreadable)
        );

        let mut other = bytes_from::<16>(0x00);
        other[15] ^= 1;
        let other = Channel::new(&LinkId::from_bytes(other), &secret());
        assert_eq!(
            other.open_offer(&sealed_offer()),
            Err(DeviceLinkError::Unreadable)
        );
        let mut wrong_secret = bytes_from::<32>(0x20);
        wrong_secret[0] ^= 1;
        let wrong = Channel::new(&link_id(), &LinkSecret::from_bytes(wrong_secret));
        assert_eq!(
            wrong.open_offer(&sealed_offer()),
            Err(DeviceLinkError::Unreadable)
        );
    }

    #[test]
    fn a_flipped_aad_nonce_tag_or_ciphertext_byte_fails() {
        let channel = code().channel();
        let mut aad_bytes = aad(&link_id(), Direction::Offer);
        assert!(
            channel
                .open_with_aad(Direction::Offer, &aad_bytes, &sealed_offer())
                .is_ok()
        );
        aad_bytes[0] ^= 1;
        assert_eq!(
            channel.open_with_aad(Direction::Offer, &aad_bytes, &sealed_offer()),
            Err(DeviceLinkError::Unreadable)
        );

        let mut flipped = sealed_offer();
        flipped.nonce[11] ^= 1;
        assert_eq!(
            channel.open(Direction::Offer, &flipped),
            Err(DeviceLinkError::Unreadable)
        );

        let mut flipped = sealed_offer();
        let last = flipped.ciphertext.len() - 1;
        flipped.ciphertext[last] ^= 1;
        assert_eq!(
            channel.open(Direction::Offer, &flipped),
            Err(DeviceLinkError::Unreadable)
        );

        let mut flipped = sealed_offer();
        flipped.ciphertext[0] ^= 1;
        assert_eq!(
            channel.open(Direction::Offer, &flipped),
            Err(DeviceLinkError::Unreadable)
        );
    }

    #[test]
    fn plaintext_and_ciphertext_caps() {
        let channel = code().channel();
        assert_eq!(Direction::Offer.max_ciphertext_len(), 16_400);
        assert_eq!(Direction::Answer.max_ciphertext_len(), 65_552);
        assert!(channel.seal(Direction::Offer, &[b'x'; 16 * 1024]).is_ok());
        assert_eq!(
            channel.seal(Direction::Offer, &[b'x'; 16 * 1024 + 1]),
            Err(DeviceLinkError::TooLarge { max: 16 * 1024 })
        );
        assert!(channel.seal(Direction::Answer, &[b'x'; 64 * 1024]).is_ok());
        assert_eq!(
            channel.seal(Direction::Answer, &[b'x'; 64 * 1024 + 1]),
            Err(DeviceLinkError::TooLarge { max: 64 * 1024 })
        );

        let too_long = SealedMessage {
            nonce: [0; NONCE_LEN],
            ciphertext: vec![0; 16_401],
        };
        assert_eq!(
            channel.open(Direction::Offer, &too_long),
            Err(DeviceLinkError::Unreadable)
        );
        let tag_only = SealedMessage {
            nonce: [0; NONCE_LEN],
            ciphertext: vec![0; TAG_LEN],
        };
        assert_eq!(
            channel.open(Direction::Offer, &tag_only),
            Err(DeviceLinkError::Unreadable)
        );
    }

    #[test]
    fn sealed_message_text_is_strict() {
        assert!(SealedMessage::from_text(OFFER_NONCE, OFFER_CIPHERTEXT).is_ok());
        for nonce in [
            format!("{OFFER_NONCE}="),
            OFFER_NONCE[..15].to_string(),
            b64(&[0; 11]),
            b64(&[0; 13]),
            "QEFCQ0RFRkdISUp+".to_string(),
        ] {
            assert_eq!(
                SealedMessage::from_text(&nonce, OFFER_CIPHERTEXT),
                Err(DeviceLinkError::BadEncoding("nonce")),
                "{nonce}"
            );
        }
        for ciphertext in [format!("{OFFER_CIPHERTEXT}="), "J8-H+".to_string()] {
            assert_eq!(
                SealedMessage::from_text(OFFER_NONCE, &ciphertext),
                Err(DeviceLinkError::BadEncoding("ciphertext"))
            );
        }
        // Non-zero trailing bits: `B` and `A` decode to the same byte, only one is canonical.
        assert!(SealedMessage::from_text(OFFER_NONCE, "AA").is_ok());
        assert!(SealedMessage::from_text(OFFER_NONCE, "AB").is_err());
    }

    #[test]
    fn code_reader_ignores_whitespace_and_scheme_case() {
        let wrapped = format!(
            " {}\n{}\r\n\t{}\x0c ",
            &CODE[..30],
            &CODE[30..70],
            &CODE[70..]
        );
        assert_eq!(LinkCode::parse(&wrapped), Ok(code()));
        // Vertical tab is not in the set.
        assert!(LinkCode::parse(&format!("{}\x0b{}", &CODE[..30], &CODE[30..])).is_err());
        let upper = CODE.replacen("rim-link:", "RIM-Link:", 1);
        assert_eq!(LinkCode::parse(&upper), Ok(code()));
        // Writers write lowercase.
        assert!(code().to_string().starts_with("rim-link:1:"));
    }

    #[test]
    fn code_reader_refuses_other_versions_and_schemes() {
        assert_eq!(
            LinkCode::parse(&CODE.replacen(":1:", ":2:", 1)),
            Err(DeviceLinkError::UnsupportedVersion)
        );
        assert_eq!(
            LinkCode::parse(&CODE.replacen(":1:", ":10:", 1)),
            Err(DeviceLinkError::UnsupportedVersion)
        );
        for version in ["0", "01", "x", "", "+1", "99999999999"] {
            assert_eq!(
                LinkCode::parse(&CODE.replacen(":1:", &format!(":{version}:"), 1)),
                Err(DeviceLinkError::NotALinkCode),
                "{version}"
            );
        }
        for text in [
            "",
            "rim-link",
            "rim-lnk:1:AAECAwQFBgcICQoLDA0ODw:x:http://a",
            "rim-link:1:AAECAwQFBgcICQoLDA0ODw",
            "rim-link:1:AAECAwQFBgcICQoLDA0ODw:ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8",
            "ržm-link:1:",
        ] {
            assert_eq!(
                LinkCode::parse(text),
                Err(DeviceLinkError::NotALinkCode),
                "{text}"
            );
        }
    }

    #[test]
    fn code_reader_bounds_the_length() {
        let fixed = format!("rim-link:1:{LINK_ID}:{SECRET}:");
        assert_eq!(fixed.len(), 78);
        let endpoint = |len: usize| format!("https://{}", "a".repeat(len - "https://".len()));
        let longest = format!("{fixed}{}", endpoint(256));
        assert_eq!(longest.len(), 334);
        assert_eq!(LinkCode::parse(&longest).map(|c| c.endpoint.len()), Ok(256));
        let over = format!("{fixed}{}", endpoint(257));
        assert_eq!(over.len(), 335);
        assert_eq!(LinkCode::parse(&over), Err(DeviceLinkError::CodeTooLong));
        // Whitespace does not count.
        assert!(LinkCode::parse(&format!("{longest}\n\n  ")).is_ok());
    }

    #[test]
    fn code_reader_decodes_link_id_and_secret_strictly() {
        let with = |link_id: &str, secret: &str| {
            LinkCode::parse(&format!("rim-link:1:{link_id}:{secret}:{ENDPOINT}"))
        };
        assert_eq!(
            with("AAECAwQFBgcICQoLDA0ODw==", SECRET),
            Err(DeviceLinkError::BadEncoding("link id"))
        );
        assert_eq!(
            with(&LINK_ID[..21], SECRET),
            Err(DeviceLinkError::BadEncoding("link id"))
        );
        // The last character carries 4 unused bits; `x` sets one of them.
        assert_eq!(
            with("AAECAwQFBgcICQoLDA0ODx", SECRET),
            Err(DeviceLinkError::BadEncoding("link id"))
        );
        assert_eq!(
            with(LINK_ID, &SECRET[..42]),
            Err(DeviceLinkError::BadEncoding("secret"))
        );
        assert_eq!(
            with(LINK_ID, &format!("{SECRET}=")),
            Err(DeviceLinkError::BadEncoding("secret"))
        );
    }

    #[test]
    fn code_reader_checks_the_endpoint() {
        let with = |endpoint: &str| {
            LinkCode::parse(&format!("rim-link:1:{LINK_ID}:{SECRET}:{endpoint}"))
                .map(|c| c.endpoint)
        };
        // Colons in the endpoint stay in the endpoint.
        assert_eq!(
            with("http://[::1]:8080/api"),
            Ok("http://[::1]:8080/api".to_string())
        );
        assert_eq!(
            with("https://rim.example.com/api"),
            Ok("https://rim.example.com/api".to_string())
        );
        for endpoint in [
            "",
            "ftp://rim.example.com",
            "rim.example.com/api",
            "https://rim.example.com/api?x=1",
            "https://rim.example.com/api#top",
            "https://user@rim.example.com/api",
            "https://rim.example.com/\u{7f}",
            "https://rím.example.com",
        ] {
            assert_eq!(
                with(endpoint),
                Err(DeviceLinkError::BadEndpoint),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn code_writer_removes_trailing_slashes_and_checks_the_endpoint() {
        let code = LinkCode::new(link_id(), secret(), "http://localhost:8080/api/").expect("code");
        assert_eq!(code.to_string(), CODE);
        assert_eq!(
            LinkCode::new(link_id(), secret(), "https://a/?q"),
            Err(DeviceLinkError::BadEndpoint)
        );
    }

    #[test]
    fn link_secret_debug_hides_the_secret() {
        assert_eq!(format!("{:?}", secret()), "LinkSecret(..)");
        assert!(!format!("{:?}", code()).contains(SECRET));
        assert_ne!(LinkSecret::generate(), LinkSecret::generate());
    }

    #[test]
    fn offer_reader_is_strict() {
        let parse = |s: &str| Offer::parse(s.as_bytes());
        assert_eq!(parse(OFFER_PLAINTEXT), Ok(offer()));
        // Field order does not matter to readers.
        let reordered = r#"{"type":"link-offer","v":1,"platform":"desktop","name":"Laptop","deviceKey":"ed25519:IIKL9cW9ystoSGMzbCAvtVmdpIvlWWYVdCFwcFvsqfc","deviceId":"00000000-0000-0000-0000-000000000001","statementVersion":2}"#;
        assert_eq!(parse(reordered), Ok(offer()));
        // The name is trimmed like a certificate's.
        assert_eq!(
            parse(&OFFER_PLAINTEXT.replace(r#""Laptop""#, r#""  Laptop ""#)).map(|o| o.name),
            Ok("Laptop".to_string())
        );

        let refused = [
            OFFER_PLAINTEXT.replace(r#""statementVersion":2}"#, r#""statementVersion":2,"x":1}"#),
            OFFER_PLAINTEXT.replace(r#""name":"Laptop","#, r#""name":"Laptop","name":"Other","#),
            OFFER_PLAINTEXT.replace("link-offer", "link-answer"),
            OFFER_PLAINTEXT.replace(r#""v":1"#, r#""v":2"#),
            OFFER_PLAINTEXT.replace(r#""statementVersion":2"#, r#""statementVersion":1"#),
            OFFER_PLAINTEXT.replace(
                "00000000-0000-0000-0000-000000000001",
                "00000000000000000000000000000001",
            ),
            OFFER_PLAINTEXT.replace("0000-000000000001", "0000-00000000000A"),
            OFFER_PLAINTEXT.replace("ed25519:", ""),
            OFFER_PLAINTEXT.replace("ed25519:", "ed25519-mldsa44:"),
            OFFER_PLAINTEXT.replace(r#""Laptop""#, &format!(r#""{}""#, "x".repeat(65))),
            OFFER_PLAINTEXT.replace(r#""Laptop""#, r#""Lap\u0007top""#),
            OFFER_PLAINTEXT.replace(r#""Laptop""#, r#""   ""#),
            OFFER_PLAINTEXT.replace("desktop", "mobile"),
            OFFER_PLAINTEXT.replace(r#","platform":"desktop""#, ""),
            "not json".to_string(),
        ];
        for text in refused {
            assert!(parse(&text).is_err(), "{text}");
        }
        assert!(Offer::parse(&[0xff, 0xfe]).is_err());
    }

    #[test]
    fn answer_reader_is_strict() {
        let (_, answer) = certified();
        assert_eq!(Answer::parse(ANSWER_PLAINTEXT.as_bytes()), Ok(answer));
        let refused = [
            ANSWER_PLAINTEXT.replace(r#""}}"#, r#"","x":1}}"#),
            ANSWER_PLAINTEXT.replacen(r#"{"v":1,"#, r#"{"v":1,"v":1,"#, 1),
            ANSWER_PLAINTEXT.replace("link-answer", "link-offer"),
            ANSWER_PLAINTEXT.replacen(r#"{"v":1,"#, r#"{"v":2,"#, 1),
            ANSWER_PLAINTEXT.replace(r#""}}"#, r#""},"x":1}"#),
        ];
        for text in refused {
            assert!(Answer::parse(text.as_bytes()).is_err(), "{text}");
        }
    }

    #[test]
    fn a_plaintext_that_breaks_the_rules_reads_like_one_that_does_not_open() {
        let channel = code().channel();
        let bogus = channel
            .seal(Direction::Offer, br#"{"v":1,"type":"link-offer"}"#)
            .expect("seal");
        assert_eq!(channel.open_offer(&bogus), Err(DeviceLinkError::Unreadable));
        let bogus = channel
            .seal(Direction::Answer, OFFER_PLAINTEXT.as_bytes())
            .expect("seal");
        assert_eq!(
            channel.open_answer(&bogus),
            Err(DeviceLinkError::Unreadable)
        );
    }

    #[test]
    fn answer_verify_refuses_a_certificate_for_another_device() {
        let (state, answer) = certified();
        let mut other_id = offer();
        other_id.device_id = Uuid::from_u128(2);
        assert!(answer.verify(&other_id, &state).is_err());
        let mut other_key = offer();
        other_key.device_key = PublicKey::from(&signing_key(21));
        assert!(answer.verify(&other_key, &state).is_err());

        // A certificate by another identity does not verify against this log.
        let stranger = TestIdentity::with_version(2, V2);
        let stranger_state = KeyState::from_inception(&stranger.inception()).expect("inception");
        assert!(answer.verify(&offer(), &stranger_state).is_err());
    }
}
