//! The server's link key: the key file that holds it, and the sealing of public join link
//! tokens under it, so a database dump alone yields no working link.
//!
//! The key file is `{"v":1,"key":"<base64url, 32 bytes>","createdAt":"…"}`, mode 0600. A
//! sealed token is a 24-byte random nonce followed by the XChaCha20-Poly1305 ciphertext of the
//! token, with the workspace id as associated data, so a sealed copy opens only for its own
//! workspace.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use rim_core::public_join_link::repository::PublicJoinLinkRepository;
use rim_core::server::model::ServerId;

use crate::secret_file::{check_private, write_secret};

/// Version of the key file format.
const KEY_FILE_VERSION: u32 = 1;
/// Length of the link key.
const KEY_LEN: usize = 32;
/// Length of a sealed token's nonce.
const NONCE_LEN: usize = 24;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct KeyFile {
    v: u32,
    key: String,
    created_at: DateTime<Utc>,
}

impl Drop for KeyFile {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

/// The key public join link tokens are sealed under. The secret is wiped from memory when the
/// last copy is dropped.
#[derive(Clone)]
pub struct LinkKey(Arc<Zeroizing<[u8; KEY_LEN]>>);

impl std::fmt::Debug for LinkKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkKey").finish_non_exhaustive()
    }
}

impl LinkKey {
    /// The key with the bytes `secret`.
    pub fn new(secret: [u8; KEY_LEN]) -> Self {
        Self(Arc::new(Zeroizing::new(secret)))
    }

    /// A new key from the OS random generator.
    ///
    /// # Errors
    /// When the OS random generator fails.
    pub fn generate() -> anyhow::Result<Self> {
        let mut secret = Zeroizing::new([0u8; KEY_LEN]);
        getrandom::fill(secret.as_mut()).map_err(|e| anyhow!("entropy failure: {e}"))?;
        Ok(Self(Arc::new(secret)))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(self.0.as_slice()))
    }

    /// `token` sealed for `server_id`: a random nonce and the ciphertext.
    ///
    /// # Errors
    /// When the OS random generator or the cipher fails.
    pub fn seal(&self, server_id: ServerId, token: &str) -> anyhow::Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|e| anyhow!("entropy failure: {e}"))?;
        let aad = uuid::Uuid::from(server_id);
        let ciphertext = self
            .cipher()
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: token.as_bytes(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| anyhow!("cannot seal the link token"))?;
        let mut sealed = nonce.to_vec();
        sealed.extend_from_slice(&ciphertext);
        Ok(sealed)
    }

    /// The token `sealed` holds for `server_id`; `None` when it was sealed under another key or
    /// for another workspace, or is damaged.
    pub fn open(&self, server_id: ServerId, sealed: &[u8]) -> Option<String> {
        if sealed.len() <= NONCE_LEN {
            return None;
        }
        let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
        let aad = uuid::Uuid::from(server_id);
        let plain = Zeroizing::new(
            self.cipher()
                .decrypt(
                    XNonce::from_slice(nonce),
                    Payload {
                        msg: ciphertext,
                        aad: aad.as_bytes(),
                    },
                )
                .ok()?,
        );
        String::from_utf8(plain.to_vec()).ok()
    }
}

/// Read and check a key file: mode 0600, the format, and a 32-byte key.
///
/// # Errors
/// When the file cannot be read, is readable by others, or is not a link key file.
pub fn read_key_file(path: &Path) -> anyhow::Result<LinkKey> {
    check_private(path)?;
    let text = Zeroizing::new(
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?,
    );
    let file: KeyFile = serde_json::from_str(&text)
        .map_err(|_| anyhow!("{} is not a link key file", path.display()))?;
    if file.v != KEY_FILE_VERSION {
        bail!("{} has unsupported version {}", path.display(), file.v);
    }
    let bytes = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(file.key.trim())
            .map_err(|_| anyhow!("{}: the key is not unpadded base64url", path.display()))?,
    );
    let secret: [u8; KEY_LEN] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("{}: the key is not {KEY_LEN} bytes", path.display()))?;
    Ok(LinkKey::new(secret))
}

fn key_file_text(key: &LinkKey, created_at: DateTime<Utc>) -> Zeroizing<String> {
    let file = KeyFile {
        v: KEY_FILE_VERSION,
        key: URL_SAFE_NO_PAD.encode(key.0.as_ref()),
        created_at,
    };
    Zeroizing::new(serde_json::to_string(&file).unwrap_or_default())
}

/// Load the key file, creating it with a new key at first boot. Refuses to start when the file
/// is missing but public links are stored, whose tokens are sealed under the lost key.
///
/// # Errors
/// When the file cannot be read or written, or is missing while links exist.
pub async fn load_or_create(
    path: &Path,
    repo: &dyn PublicJoinLinkRepository,
) -> anyhow::Result<LinkKey> {
    if path.exists() {
        return read_key_file(path);
    }
    let links = repo
        .count_links()
        .await
        .map_err(|e| anyhow!("cannot count the public join links: {e}"))?;
    if links > 0 {
        bail!(
            "{} is missing but {links} public join links are sealed under it; restore the file",
            path.display()
        );
    }
    let key = LinkKey::generate()?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    match write_secret(path, &key_file_text(&key, Utc::now())) {
        Ok(()) => {}
        // Another node of the same deployment created it first.
        Err(_) if path.exists() => return read_key_file(path),
        Err(e) => return Err(e),
    }
    tracing::info!(path = %path.display(), "created the link key file");
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::public_join_link::repository::MockPublicJoinLinkRepository;

    fn counting(links: u64) -> MockPublicJoinLinkRepository {
        let mut repo = MockPublicJoinLinkRepository::new();
        repo.expect_count_links()
            .returning(move || Box::pin(async move { Ok(links) }));
        repo
    }

    #[test]
    fn a_sealed_token_opens_only_for_its_workspace_and_key() {
        let key = LinkKey::new([7; KEY_LEN]);
        let server = ServerId::new();
        let sealed = key.seal(server, "abc").unwrap();
        assert_eq!(key.open(server, &sealed).as_deref(), Some("abc"));
        assert_ne!(
            key.seal(server, "abc").unwrap(),
            sealed,
            "a fresh nonce each time"
        );
        assert_eq!(key.open(ServerId::new(), &sealed), None);
        assert_eq!(LinkKey::new([8; KEY_LEN]).open(server, &sealed), None);
        assert_eq!(key.open(server, &sealed[..NONCE_LEN]), None);
        let mut damaged = sealed.clone();
        if let Some(last) = damaged.last_mut() {
            *last ^= 1;
        }
        assert_eq!(key.open(server, &damaged), None);
    }

    #[tokio::test]
    async fn the_first_start_creates_a_private_key_file_that_later_starts_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data").join("link-key.json");
        let key = load_or_create(&path, &counting(0)).await.unwrap();
        let server = ServerId::new();
        let sealed = key.seal(server, "token").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let again = load_or_create(&path, &counting(3)).await.unwrap();
        assert_eq!(again.open(server, &sealed).as_deref(), Some("token"));
    }

    #[tokio::test]
    async fn a_missing_key_file_with_stored_links_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("link-key.json");
        let err = load_or_create(&path, &counting(2)).await.unwrap_err();
        assert!(format!("{err:#}").contains("restore the file"), "{err:#}");
        assert!(!path.exists());
    }

    #[test]
    fn a_damaged_or_open_key_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("link-key.json");
        write_secret(
            &path,
            "{\"v\":1,\"key\":\"short\",\"createdAt\":\"2026-10-23T00:00:00Z\"}",
        )
        .unwrap();
        assert!(read_key_file(&path).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let good = dir.path().join("open.json");
            write_secret(
                &good,
                &key_file_text(&LinkKey::new([1; KEY_LEN]), Utc::now()),
            )
            .unwrap();
            assert!(read_key_file(&good).is_ok());
            fs::set_permissions(&good, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read_key_file(&good).is_err());
        }
    }
}
