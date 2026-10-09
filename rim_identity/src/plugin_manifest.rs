use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::encoding::PublicKey;
use crate::error::{IdentityError, IdentityResult};
use crate::key_log::{KeyState, RotationSigner};
use crate::statement::{
    Purpose, SignedStatement, StatementVersion, check_header, check_label, parse, verify,
};

/// Longest `publisherName`, in characters.
pub const MAX_PUBLISHER_NAME_CHARS: usize = 80;
/// Longest plugin `version`, in characters.
pub const MAX_PLUGIN_VERSION_CHARS: usize = 64;
/// Largest embedded `manifest`, in bytes.
pub const MAX_MANIFEST_LEN: usize = 64 * 1024;
/// Largest bundle a `bundle` source may name, in bytes.
pub const MAX_BUNDLE_SIZE: u64 = 8 * 1024 * 1024;
/// Longest `git` source URL, in bytes.
pub const MAX_GIT_URL_LEN: usize = 2048;

/// One entry of a replayed key log: the identity key it made current and who signed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyHistoryEntry {
    pub seq: u32,
    pub identity_key: PublicKey,
    /// `None` for the inception.
    pub signer: Option<RotationSigner>,
}

/// A key log verified from its inception to its head, keeping every entry's identity key and
/// signer, so a statement signed under an older entry can be checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyHistory {
    head: KeyState,
    entries: Vec<KeyHistoryEntry>,
}

impl KeyHistory {
    /// Verify a whole log, oldest entry first.
    pub fn replay(entries: &[SignedStatement]) -> IdentityResult<Self> {
        let (first, rest) = entries
            .split_first()
            .ok_or_else(|| IdentityError::Invalid("key log is empty".to_string()))?;
        let mut head = KeyState::from_inception(first)?;
        let mut history = Vec::with_capacity(entries.len());
        history.push(KeyHistoryEntry {
            seq: 0,
            identity_key: head.identity_key.clone(),
            signer: None,
        });
        for entry in rest {
            let (next, signer) = head.apply(entry)?;
            history.push(KeyHistoryEntry {
                seq: next.seq,
                identity_key: next.identity_key.clone(),
                signer: Some(signer),
            });
            head = next;
        }
        Ok(Self {
            head,
            entries: history,
        })
    }

    pub fn principal_id(&self) -> &str {
        &self.head.principal_id
    }

    /// The state at the head of the log.
    pub fn head(&self) -> &KeyState {
        &self.head
    }

    /// Every entry, oldest first; an entry's index is its seq.
    pub fn entries(&self) -> &[KeyHistoryEntry] {
        &self.entries
    }

    pub fn entry(&self, seq: u32) -> Option<&KeyHistoryEntry> {
        self.entries.get(seq as usize)
    }

    /// The seq of the first recovery rotation after entry `seq`, if there is one.
    pub fn recovery_after(&self, seq: u32) -> Option<u32> {
        self.entries
            .iter()
            .skip(seq as usize + 1)
            .find(|e| e.signer == Some(RotationSigner::Recovery))
            .map(|e| e.seq)
    }
}

/// The two kinds of plugin a manifest's `runtime_kind` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginRuntimeKind {
    DesktopLocalProcess,
    WebSandboxed,
}

impl PluginRuntimeKind {
    /// The value of the manifest's `runtime_kind`.
    pub fn name(self) -> &'static str {
        match self {
            Self::DesktopLocalProcess => "DesktopLocalProcess",
            Self::WebSandboxed => "WebSandboxed",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "DesktopLocalProcess" => Some(Self::DesktopLocalProcess),
            "WebSandboxed" => Some(Self::WebSandboxed),
            _ => None,
        }
    }
}

/// Where the signed plugin's code comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    /// A bundle file: its SHA-256 as lowercase hex, and its length in bytes.
    Bundle { sha256: String, size: u64 },
    /// A git commit, and the SHA-256 of its tree's files as lowercase hex.
    Git {
        url: String,
        commit: String,
        tree_sha256: String,
    },
}

/// A plugin manifest statement whose fields passed every rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifestStatement {
    pub principal_id: String,
    pub key_seq: u32,
    /// Trimmed. Chosen by the publisher, so it proves nothing about who they are.
    pub publisher_name: String,
    pub plugin_id: String,
    pub version: String,
    /// The exact text of `rim-plugin.json`.
    pub manifest: String,
    /// The fields of the manifest a registry stores; the host checks the rest.
    pub manifest_version: u64,
    pub display_name: String,
    pub runtime_kind: PluginRuntimeKind,
    pub source: PluginSource,
    pub issued_at: DateTime<Utc>,
}

/// Whether a verified statement still stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validity {
    Valid,
    /// The key log entry at `at_seq`, after the statement's `keySeq`, is a recovery rotation:
    /// the identity key that signed may be in someone else's hands.
    PublisherRecovered {
        at_seq: u32,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawSource {
    kind: String,
    sha256: Option<String>,
    size: Option<u64>,
    url: Option<String>,
    commit: Option<String>,
    tree_sha256: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawPluginManifest {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    principal_id: String,
    key_seq: u32,
    publisher_name: String,
    plugin_id: String,
    version: String,
    manifest: String,
    source: RawSource,
    issued_at: DateTime<Utc>,
}

/// The manifest fields a statement is checked against. Other fields are the host's.
#[derive(Deserialize)]
struct ManifestHead {
    manifest_version: u64,
    plugin_id: String,
    display_name: String,
    version: String,
    runtime_kind: String,
}

fn invalid(message: impl Into<String>) -> IdentityError {
    IdentityError::Invalid(message.into())
}

fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn check_git_url(url: &str) -> IdentityResult<()> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("ssh://"));
    let ok = rest.is_some_and(|r| !r.is_empty())
        && url.len() <= MAX_GIT_URL_LEN
        && !url.chars().any(|c| c.is_whitespace() || c.is_control());
    if !ok {
        return Err(invalid(format!(
            "source.url must be an https:// or ssh:// URL of at most {MAX_GIT_URL_LEN} bytes"
        )));
    }
    Ok(())
}

fn read_source(raw: RawSource) -> IdentityResult<PluginSource> {
    match raw {
        RawSource {
            kind,
            sha256: Some(sha256),
            size: Some(size),
            url: None,
            commit: None,
            tree_sha256: None,
        } if kind == "bundle" => {
            if !is_lower_hex(&sha256, 64) {
                return Err(invalid("source.sha256 must be 64 lowercase hex characters"));
            }
            if !(1..=MAX_BUNDLE_SIZE).contains(&size) {
                return Err(invalid(format!(
                    "source.size must be 1 to {MAX_BUNDLE_SIZE}"
                )));
            }
            Ok(PluginSource::Bundle { sha256, size })
        }
        RawSource {
            kind,
            sha256: None,
            size: None,
            url: Some(url),
            commit: Some(commit),
            tree_sha256: Some(tree_sha256),
        } if kind == "git" => {
            check_git_url(&url)?;
            if !is_lower_hex(&commit, 40) && !is_lower_hex(&commit, 64) {
                return Err(invalid(
                    "source.commit must be 40 or 64 lowercase hex characters",
                ));
            }
            if !is_lower_hex(&tree_sha256, 64) {
                return Err(invalid(
                    "source.treeSha256 must be 64 lowercase hex characters",
                ));
            }
            Ok(PluginSource::Git {
                url,
                commit,
                tree_sha256,
            })
        }
        _ => Err(invalid(
            "source must be a bundle (sha256, size) or a git commit (url, commit, treeSha256)",
        )),
    }
}

fn read_manifest(manifest: &str) -> IdentityResult<ManifestHead> {
    if manifest.len() > MAX_MANIFEST_LEN {
        return Err(invalid(format!(
            "manifest must be at most {MAX_MANIFEST_LEN} bytes"
        )));
    }
    // A struct also deserializes from a JSON array, so the object is checked by its first byte.
    if !manifest.trim_start().starts_with('{') {
        return Err(invalid("manifest must be a JSON object"));
    }
    serde_json::from_str(manifest).map_err(|e| IdentityError::Malformed(format!("manifest: {e}")))
}

/// Read a plugin manifest statement and check every field rule, without its signature.
///
/// For statements read back from storage, whose signature was verified when they were stored.
/// Anything received runs through [`verify_plugin_manifest`].
pub fn read_plugin_manifest(
    statement: &SignedStatement,
) -> IdentityResult<PluginManifestStatement> {
    let raw: RawPluginManifest = parse(Purpose::PluginManifest, &statement.payload)?;
    let version = check_header(raw.v, &raw.kind, "plugin-manifest")?;
    if version != StatementVersion::V2 {
        return Err(invalid("a plugin manifest must be a version 2 statement"));
    }
    let publisher_name = check_label(
        &raw.publisher_name,
        "publisherName",
        MAX_PUBLISHER_NAME_CHARS,
    )?;
    let head = read_manifest(&raw.manifest)?;
    if head.plugin_id != raw.plugin_id {
        return Err(invalid("pluginId does not match the manifest"));
    }
    if head.version != raw.version {
        return Err(invalid("version does not match the manifest"));
    }
    let version_chars = raw.version.chars().count();
    if !(1..=MAX_PLUGIN_VERSION_CHARS).contains(&version_chars) {
        return Err(invalid(format!(
            "version must be 1 to {MAX_PLUGIN_VERSION_CHARS} characters"
        )));
    }
    let runtime_kind = PluginRuntimeKind::from_name(&head.runtime_kind).ok_or_else(|| {
        invalid("the manifest's runtime_kind must be DesktopLocalProcess or WebSandboxed")
    })?;
    let source = read_source(raw.source)?;
    if runtime_kind == PluginRuntimeKind::WebSandboxed
        && !matches!(source, PluginSource::Bundle { .. })
    {
        return Err(invalid(
            "a WebSandboxed plugin must come with a bundle source",
        ));
    }
    Ok(PluginManifestStatement {
        principal_id: raw.principal_id,
        key_seq: raw.key_seq,
        publisher_name,
        plugin_id: raw.plugin_id,
        version: raw.version,
        manifest: raw.manifest,
        manifest_version: head.manifest_version,
        display_name: head.display_name,
        runtime_kind,
        source,
        issued_at: raw.issued_at,
    })
}

/// Verify a plugin manifest statement against its publisher's key log.
///
/// Every field rule applies; the principal must be the log's, `keySeq` an entry of it, and the
/// signature that entry's identity key's. A statement that verifies is still answered with
/// [`Validity::PublisherRecovered`] when a recovery rotation follows its `keySeq`; whether to
/// accept it then is the caller's decision.
pub fn verify_plugin_manifest(
    history: &KeyHistory,
    statement: &SignedStatement,
) -> IdentityResult<(PluginManifestStatement, Validity)> {
    let parsed = read_plugin_manifest(statement)?;
    if parsed.principal_id != history.principal_id() {
        return Err(IdentityError::PrincipalMismatch);
    }
    let entry = history.entry(parsed.key_seq).ok_or_else(|| {
        invalid(format!(
            "keySeq {} is not an entry of the publisher's key log",
            parsed.key_seq
        ))
    })?;
    verify(
        Purpose::PluginManifest,
        StatementVersion::V2,
        &entry.identity_key,
        statement,
    )?;
    let validity = match history.recovery_after(parsed.key_seq) {
        Some(at_seq) => Validity::PublisherRecovered { at_seq },
        None => Validity::Valid,
    };
    Ok((parsed, validity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statement::StatementVersion::V2;
    use crate::statement::sign;
    use crate::testing::{
        KNOWN_PLUGIN_BUNDLE, KNOWN_PLUGIN_MANIFEST_PAYLOAD, KNOWN_PLUGIN_MANIFEST_SIGNATURE,
        TestIdentity, bundle_source, plugin_manifest_payload, signing_key, web_manifest,
    };

    const SHA: &str = "5b154b3eb45c3ebc44f6820ba30a389ce85b43dea34bcbf652b9b1a5416fbe85";

    fn publisher() -> (TestIdentity, SignedStatement, KeyHistory) {
        let id = TestIdentity::with_version(1, V2);
        let inception = id.inception();
        let history = KeyHistory::replay(std::slice::from_ref(&inception)).expect("replay");
        (id, inception, history)
    }

    fn signed(payload: String) -> SignedStatement {
        sign(Purpose::PluginManifest, V2, &signing_key(1), payload)
    }

    fn payload_with(key_seq: u32, manifest: &str, source: &str) -> String {
        plugin_manifest_payload(
            &TestIdentity::with_version(1, V2).principal_id(),
            key_seq,
            "Ada",
            manifest,
            source,
        )
    }

    fn web_payload() -> String {
        payload_with(
            0,
            &web_manifest("dev.example.hello-web", "0.1.0"),
            &bundle_source(SHA, 1605),
        )
    }

    fn refused(history: &KeyHistory, payload: String) -> IdentityError {
        verify_plugin_manifest(history, &signed(payload)).expect_err("must be refused")
    }

    fn known() -> SignedStatement {
        SignedStatement {
            payload: KNOWN_PLUGIN_MANIFEST_PAYLOAD.to_string(),
            signature: KNOWN_PLUGIN_MANIFEST_SIGNATURE.to_string(),
        }
    }

    #[test]
    fn known_answer_statement_verifies_against_the_known_inception() {
        let (_, _, history) = publisher();
        let (statement, validity) = verify_plugin_manifest(&history, &known()).expect("verify");
        assert_eq!(validity, Validity::Valid);
        assert_eq!(statement.plugin_id, "dev.example.hello-web");
        assert_eq!(statement.version, "0.1.0");
        assert_eq!(statement.publisher_name, "Ada");
        assert_eq!(statement.display_name, "Hello Web");
        assert_eq!(statement.runtime_kind, PluginRuntimeKind::WebSandboxed);
        assert_eq!(
            statement.source,
            PluginSource::Bundle {
                sha256: SHA.to_string(),
                size: 1605
            }
        );
        // The test helpers write the known payload byte for byte.
        assert_eq!(web_payload(), KNOWN_PLUGIN_MANIFEST_PAYLOAD);
    }

    #[test]
    fn known_answer_bundle_matches_the_statement() {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(KNOWN_PLUGIN_BUNDLE);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, SHA);
        assert_eq!(KNOWN_PLUGIN_BUNDLE.len(), 1605);
        let bundle: serde_json::Value =
            serde_json::from_slice(KNOWN_PLUGIN_BUNDLE).expect("bundle");
        let manifest = bundle["files"][1]["text"].as_str().expect("manifest");
        let statement = read_plugin_manifest(&known()).expect("read");
        assert_eq!(manifest, statement.manifest);
    }

    #[test]
    fn flipped_payload_byte_fails_the_signature() {
        let (_, _, history) = publisher();
        let mut statement = known();
        statement.payload = statement.payload.replace("\"Ada\"", "\"Adb\"");
        assert_eq!(
            verify_plugin_manifest(&history, &statement),
            Err(IdentityError::BadSignature)
        );
    }

    #[test]
    fn signature_under_another_context_fails() {
        let (_, _, history) = publisher();
        let statement = sign(Purpose::KeyLog, V2, &signing_key(1), web_payload());
        assert_eq!(
            verify_plugin_manifest(&history, &statement),
            Err(IdentityError::BadSignature)
        );
    }

    #[test]
    fn version_1_is_refused() {
        let (_, _, history) = publisher();
        let payload = web_payload().replacen(r#""v":2"#, r#""v":1"#, 1);
        assert!(matches!(
            refused(&history, payload),
            IdentityError::Invalid(_)
        ));
    }

    #[test]
    fn unknown_and_duplicate_fields_are_refused() {
        let (_, _, history) = publisher();
        let unknown = web_payload().replacen('{', r#"{"extra":1,"#, 1);
        assert!(matches!(
            refused(&history, unknown),
            IdentityError::Malformed(_)
        ));
        let duplicate = web_payload().replacen('{', r#"{"publisherName":"Eve","#, 1);
        assert!(matches!(
            refused(&history, duplicate),
            IdentityError::Malformed(_)
        ));
        let unknown_source =
            web_payload().replacen(r#""size":1605"#, r#""size":1605,"mirror":"x""#, 1);
        assert!(matches!(
            refused(&history, unknown_source),
            IdentityError::Malformed(_)
        ));
    }

    #[test]
    fn wrong_type_is_refused() {
        let (_, _, history) = publisher();
        let payload = web_payload().replacen("plugin-manifest", "profile-card", 1);
        assert_eq!(
            refused(&history, payload),
            IdentityError::WrongType {
                expected: "plugin-manifest"
            }
        );
    }

    #[test]
    fn key_seq_past_the_log_is_refused() {
        let (_, _, history) = publisher();
        let payload = web_payload().replacen(r#""keySeq":0"#, r#""keySeq":1"#, 1);
        assert_eq!(
            refused(&history, payload).to_string(),
            "keySeq 1 is not an entry of the publisher's key log"
        );
    }

    #[test]
    fn another_principal_is_refused() {
        let (_, _, history) = publisher();
        let other = TestIdentity::with_version(5, V2).principal_id();
        let payload = plugin_manifest_payload(
            &other,
            0,
            "Ada",
            &web_manifest("dev.example.hello-web", "0.1.0"),
            &bundle_source(SHA, 1605),
        );
        assert_eq!(refused(&history, payload), IdentityError::PrincipalMismatch);
    }

    #[test]
    fn plugin_id_or_version_unlike_the_manifest_is_refused() {
        let (_, _, history) = publisher();
        let manifest = web_manifest("dev.example.hello-web", "0.1.0");
        let source = bundle_source(SHA, 1605);
        let payload = payload_with(0, &manifest, &source).replacen(
            r#""pluginId":"dev.example.hello-web""#,
            r#""pluginId":"dev.example.other""#,
            1,
        );
        assert_eq!(
            refused(&history, payload).to_string(),
            "pluginId does not match the manifest"
        );
        let payload = payload_with(0, &manifest, &source).replacen(
            r#""version":"0.1.0""#,
            r#""version":"0.2.0""#,
            1,
        );
        assert_eq!(
            refused(&history, payload).to_string(),
            "version does not match the manifest"
        );
    }

    #[test]
    fn web_manifest_with_a_git_source_is_refused() {
        let (_, _, history) = publisher();
        let git = format!(
            r#"{{"kind":"git","url":"https://github.com/acme/hello","commit":"{}","treeSha256":"{SHA}"}}"#,
            "a".repeat(40)
        );
        let payload = payload_with(0, &web_manifest("dev.example.hello-web", "0.1.0"), &git);
        assert_eq!(
            refused(&history, payload).to_string(),
            "a WebSandboxed plugin must come with a bundle source"
        );

        // The same source is fine for a desktop plugin.
        let desktop = web_manifest("dev.example.hello-web", "0.1.0")
            .replace("WebSandboxed", "DesktopLocalProcess");
        let statement = signed(payload_with(0, &desktop, &git));
        let (parsed, _) = verify_plugin_manifest(&history, &statement).expect("desktop git");
        assert!(matches!(parsed.source, PluginSource::Git { .. }));
    }

    #[test]
    fn bundle_size_out_of_range_is_refused() {
        let (_, _, history) = publisher();
        let manifest = web_manifest("dev.example.hello-web", "0.1.0");
        for size in [0, MAX_BUNDLE_SIZE + 1] {
            let payload = payload_with(0, &manifest, &bundle_source(SHA, size));
            assert_eq!(
                refused(&history, payload).to_string(),
                "source.size must be 1 to 8388608"
            );
        }
        let payload = payload_with(0, &manifest, &bundle_source(SHA, MAX_BUNDLE_SIZE));
        assert!(verify_plugin_manifest(&history, &signed(payload)).is_ok());
    }

    #[test]
    fn uppercase_hex_is_refused() {
        let (_, _, history) = publisher();
        let payload = payload_with(
            0,
            &web_manifest("dev.example.hello-web", "0.1.0"),
            &bundle_source(&SHA.to_uppercase(), 1605),
        );
        assert_eq!(
            refused(&history, payload).to_string(),
            "source.sha256 must be 64 lowercase hex characters"
        );
    }

    #[test]
    fn manifest_over_64_kib_is_refused() {
        let (_, _, history) = publisher();
        let padding = "x".repeat(MAX_MANIFEST_LEN);
        let manifest = web_manifest("dev.example.hello-web", "0.1.0").replacen(
            '{',
            &format!(r#"{{"description":"{padding}","#),
            1,
        );
        let payload = payload_with(0, &manifest, &bundle_source(SHA, 1605));
        assert_eq!(
            refused(&history, payload).to_string(),
            "manifest must be at most 65536 bytes"
        );
    }

    #[test]
    fn manifest_that_is_not_an_object_is_refused() {
        let (_, _, history) = publisher();
        let manifest = serde_json::Value::from(web_manifest("dev.example.hello-web", "0.1.0"));
        let payload = web_payload().replacen(&manifest.to_string(), r#""[1]""#, 1);
        assert_eq!(
            refused(&history, payload).to_string(),
            "manifest must be a JSON object"
        );
    }

    #[test]
    fn publisher_name_follows_the_label_rules() {
        let (_, _, history) = publisher();
        let payload = web_payload().replacen(r#""Ada""#, r#"" Ada ""#, 1);
        let (parsed, _) = verify_plugin_manifest(&history, &signed(payload)).expect("trimmed");
        assert_eq!(parsed.publisher_name, "Ada");
        for name in ["  ", &"n".repeat(81), "A\\u0007"] {
            let payload = web_payload().replacen(r#""Ada""#, &format!("\"{name}\""), 1);
            assert!(matches!(
                refused(&history, payload),
                IdentityError::Invalid(_)
            ));
        }
    }

    #[test]
    fn identity_rotation_keeps_the_statement_valid() {
        let (mut id, inception, history) = publisher();
        let rotation = id.rotate_with_identity(history.head(), 2);
        let history = KeyHistory::replay(&[inception, rotation]).expect("replay");
        let (_, validity) = verify_plugin_manifest(&history, &known()).expect("verify");
        assert_eq!(validity, Validity::Valid);
        assert_eq!(history.entries()[1].signer, Some(RotationSigner::Identity));

        // A statement at the new keySeq is signed by the new identity key.
        let payload = web_payload().replacen(r#""keySeq":0"#, r#""keySeq":1"#, 1);
        let at_head = sign(Purpose::PluginManifest, V2, &signing_key(2), payload);
        assert!(verify_plugin_manifest(&history, &at_head).is_ok());
    }

    #[test]
    fn recovery_rotation_flips_the_statement() {
        let (mut id, inception, history) = publisher();
        let r1 = id.rotate_with_identity(history.head(), 2);
        let state1 = history.head().apply(&r1).expect("r1").0;
        let r2 = id.recover(&state1, 3);
        let history = KeyHistory::replay(&[inception, r1, r2]).expect("replay");
        let (_, validity) = verify_plugin_manifest(&history, &known()).expect("verify");
        assert_eq!(validity, Validity::PublisherRecovered { at_seq: 2 });
        assert_eq!(history.recovery_after(1), Some(2));
        assert_eq!(history.recovery_after(2), None);
    }
}
