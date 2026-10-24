//! The server's password key: the key file that holds the OPAQUE server setup and the share
//! key, and the sealing of account server shares, keep keys and reload keys under it, so a
//! database dump alone opens none of them.
//!
//! The key file is
//! `{"v":1,"opaqueSetup":"<base64url, 128 bytes>","shareKey":"<base64url, 32 bytes>","createdAt":"…"}`,
//! mode 0600. A sealed value is a 24-byte random nonce followed by the XChaCha20-Poly1305
//! ciphertext, with the owner's id as associated data: the user id for a server share,
//! `keep:` and the session id for a keep key, `reload:` and the session id for a reload key.

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

use rim_auth::opaque::{
    SERVER_SETUP_LEN, ServerSetup, generate_server_setup, server_setup_bytes,
    server_setup_from_bytes,
};
use rim_core::account_password::repository::AccountPasswordRepository;
use rim_core::auth::model::SessionId;
use rim_core::user::model::UserId;

use crate::secret_file::{check_private, write_secret};

/// Version of the key file format.
const KEY_FILE_VERSION: u32 = 1;
/// Length of the share key.
const KEY_LEN: usize = 32;
/// Length of a sealed value's nonce.
const NONCE_LEN: usize = 24;
/// Associated data that tells a sealed keep key from a sealed server share.
const KEEP_DOMAIN: &[u8] = b"keep:";
/// Associated data that tells a sealed reload key from a sealed keep key or server share.
const RELOAD_DOMAIN: &[u8] = b"reload:";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct KeyFile {
    v: u32,
    opaque_setup: String,
    share_key: String,
    created_at: DateTime<Utc>,
}

impl Drop for KeyFile {
    fn drop(&mut self) {
        self.opaque_setup.zeroize();
        self.share_key.zeroize();
    }
}

/// The OPAQUE server setup account passwords are registered and checked with, and the key
/// server shares, keep keys and reload keys are sealed under. The share key is wiped from memory when the
/// last copy is dropped.
#[derive(Clone)]
pub struct PasswordKey {
    setup: Arc<ServerSetup>,
    share_key: Arc<Zeroizing<[u8; KEY_LEN]>>,
}

impl std::fmt::Debug for PasswordKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordKey").finish_non_exhaustive()
    }
}

impl PasswordKey {
    /// The key made of `setup` and the share key `share_key`.
    pub fn new(setup: ServerSetup, share_key: [u8; KEY_LEN]) -> Self {
        Self {
            setup: Arc::new(setup),
            share_key: Arc::new(Zeroizing::new(share_key)),
        }
    }

    /// A new key: a new server setup and share key from the OS random generator.
    ///
    /// # Errors
    /// When the OS random generator fails.
    pub fn generate() -> anyhow::Result<Self> {
        let mut share_key = Zeroizing::new([0u8; KEY_LEN]);
        getrandom::fill(share_key.as_mut()).map_err(|e| anyhow!("entropy failure: {e}"))?;
        Ok(Self {
            setup: Arc::new(generate_server_setup()),
            share_key: Arc::new(share_key),
        })
    }

    /// The OPAQUE server setup.
    pub fn setup(&self) -> &ServerSetup {
        &self.setup
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(chacha20poly1305::Key::from_slice(self.share_key.as_slice()))
    }

    fn seal(&self, aad: &[u8], value: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|e| anyhow!("entropy failure: {e}"))?;
        let ciphertext = self
            .cipher()
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: value, aad })
            .map_err(|_| anyhow!("cannot seal under the password key"))?;
        let mut sealed = nonce.to_vec();
        sealed.extend_from_slice(&ciphertext);
        Ok(sealed)
    }

    fn open(&self, aad: &[u8], sealed: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
        if sealed.len() <= NONCE_LEN {
            return None;
        }
        let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
        self.cipher()
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .ok()
            .map(Zeroizing::new)
    }

    /// The server share `share` sealed for `user_id`.
    ///
    /// # Errors
    /// When the OS random generator or the cipher fails.
    pub fn seal_share(&self, user_id: UserId, share: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.seal(uuid::Uuid::from(user_id).as_bytes(), share)
    }

    /// The server share `sealed` holds for `user_id`; `None` when it was sealed under another
    /// key or for another account, or is damaged.
    pub fn open_share(&self, user_id: UserId, sealed: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
        self.open(uuid::Uuid::from(user_id).as_bytes(), sealed)
    }

    fn session_aad(domain: &[u8], session_id: SessionId) -> Vec<u8> {
        let mut aad = domain.to_vec();
        aad.extend_from_slice(uuid::Uuid::from(session_id).as_bytes());
        aad
    }

    /// The keep key `keep_key` sealed for `session_id`.
    ///
    /// # Errors
    /// When the OS random generator or the cipher fails.
    pub fn seal_keep(&self, session_id: SessionId, keep_key: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.seal(&Self::session_aad(KEEP_DOMAIN, session_id), keep_key)
    }

    /// The keep key `sealed` holds for `session_id`; `None` when it was sealed under another
    /// key or for another session, or is damaged.
    pub fn open_keep(&self, session_id: SessionId, sealed: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
        self.open(&Self::session_aad(KEEP_DOMAIN, session_id), sealed)
    }

    /// The reload key `reload_key` sealed for `session_id`.
    ///
    /// # Errors
    /// When the OS random generator or the cipher fails.
    pub fn seal_reload(&self, session_id: SessionId, reload_key: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.seal(&Self::session_aad(RELOAD_DOMAIN, session_id), reload_key)
    }

    /// The reload key `sealed` holds for `session_id`; `None` when it was sealed under another
    /// key or for another session, or is damaged.
    pub fn open_reload(&self, session_id: SessionId, sealed: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
        self.open(&Self::session_aad(RELOAD_DOMAIN, session_id), sealed)
    }
}

fn decode(path: &Path, field: &str, text: &str, len: usize) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    let bytes = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(text.trim())
            .map_err(|_| anyhow!("{}: {field} is not unpadded base64url", path.display()))?,
    );
    if bytes.len() != len {
        bail!("{}: {field} is not {len} bytes", path.display());
    }
    Ok(bytes)
}

/// Read and check a key file: mode 0600, the format, a 128-byte server setup and a 32-byte
/// share key.
///
/// # Errors
/// When the file cannot be read, is readable by others, or is not a password key file.
pub fn read_key_file(path: &Path) -> anyhow::Result<PasswordKey> {
    check_private(path)?;
    let text = Zeroizing::new(
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?,
    );
    let file: KeyFile = serde_json::from_str(&text)
        .map_err(|_| anyhow!("{} is not a password key file", path.display()))?;
    if file.v != KEY_FILE_VERSION {
        bail!("{} has unsupported version {}", path.display(), file.v);
    }
    let setup = decode(path, "opaqueSetup", &file.opaque_setup, SERVER_SETUP_LEN)?;
    let setup = server_setup_from_bytes(&setup)
        .map_err(|_| anyhow!("{}: opaqueSetup is not an OPAQUE setup", path.display()))?;
    let share_key = decode(path, "shareKey", &file.share_key, KEY_LEN)?;
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(&share_key);
    let password_key = PasswordKey::new(setup, key);
    key.zeroize();
    Ok(password_key)
}

fn key_file_text(key: &PasswordKey, created_at: DateTime<Utc>) -> Zeroizing<String> {
    let file = KeyFile {
        v: KEY_FILE_VERSION,
        opaque_setup: URL_SAFE_NO_PAD.encode(server_setup_bytes(key.setup()).as_slice()),
        share_key: URL_SAFE_NO_PAD.encode(key.share_key.as_ref()),
        created_at,
    };
    Zeroizing::new(serde_json::to_string(&file).unwrap_or_default())
}

/// Load the key file, creating it with a new key at first boot. Refuses to start when the file
/// is missing but passwords are stored: their records need the lost setup and their shares are
/// sealed under the lost key.
///
/// # Errors
/// When the file cannot be read or written, or is missing while passwords exist.
pub async fn load_or_create(
    path: &Path,
    repo: &dyn AccountPasswordRepository,
) -> anyhow::Result<PasswordKey> {
    if path.exists() {
        return read_key_file(path);
    }
    let passwords = repo
        .count_passwords()
        .await
        .map_err(|e| anyhow!("cannot count the account passwords: {e}"))?;
    if passwords > 0 {
        bail!(
            "{} is missing but {passwords} account passwords depend on it; restore the file",
            path.display()
        );
    }
    let key = PasswordKey::generate()?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    match write_secret(path, &key_file_text(&key, Utc::now())) {
        Ok(()) => {}
        // Another node of the same deployment created it first.
        Err(_) if path.exists() => return read_key_file(path),
        Err(e) => return Err(e),
    }
    tracing::info!(path = %path.display(), "created the password key file");
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::account_password::repository::MockAccountPasswordRepository;

    fn counting(passwords: u64) -> MockAccountPasswordRepository {
        let mut repo = MockAccountPasswordRepository::new();
        repo.expect_count_passwords()
            .returning(move || Box::pin(async move { Ok(passwords) }));
        repo
    }

    #[test]
    fn a_sealed_share_opens_only_for_its_account_and_key() {
        let key = PasswordKey::generate().unwrap();
        let user = UserId::new();
        let sealed = key.seal_share(user, &[9; 32]).unwrap();
        assert_eq!(
            key.open_share(user, &sealed).as_deref().map(Vec::as_slice),
            Some(&[9u8; 32][..])
        );
        assert_ne!(key.seal_share(user, &[9; 32]).unwrap(), sealed);
        assert!(key.open_share(UserId::new(), &sealed).is_none());
        assert!(
            PasswordKey::generate()
                .unwrap()
                .open_share(user, &sealed)
                .is_none()
        );
        assert!(key.open_share(user, &sealed[..NONCE_LEN]).is_none());
        let mut damaged = sealed.clone();
        if let Some(last) = damaged.last_mut() {
            *last ^= 1;
        }
        assert!(key.open_share(user, &damaged).is_none());
    }

    #[test]
    fn a_sealed_keep_key_opens_only_for_its_session_and_never_as_a_share() {
        let key = PasswordKey::generate().unwrap();
        let session = SessionId::new();
        let sealed = key.seal_keep(session, &[4; 32]).unwrap();
        assert_eq!(
            key.open_keep(session, &sealed)
                .as_deref()
                .map(Vec::as_slice),
            Some(&[4u8; 32][..])
        );
        assert!(key.open_keep(SessionId::new(), &sealed).is_none());
        let same_id = UserId::from(uuid::Uuid::from(session));
        assert!(key.open_share(same_id, &sealed).is_none());
    }

    #[test]
    fn a_sealed_reload_key_opens_only_for_its_session_and_never_as_a_keep_key() {
        let key = PasswordKey::generate().unwrap();
        let session = SessionId::new();
        let sealed = key.seal_reload(session, &[5; 32]).unwrap();
        assert_eq!(
            key.open_reload(session, &sealed)
                .as_deref()
                .map(Vec::as_slice),
            Some(&[5u8; 32][..])
        );
        assert!(key.open_reload(SessionId::new(), &sealed).is_none());
        assert!(key.open_keep(session, &sealed).is_none());
        let keep = key.seal_keep(session, &[5; 32]).unwrap();
        assert!(key.open_reload(session, &keep).is_none());
    }

    #[tokio::test]
    async fn the_first_start_creates_a_private_key_file_that_later_starts_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data").join("password-key.json");
        let key = load_or_create(&path, &counting(0)).await.unwrap();
        let user = UserId::new();
        let sealed = key.seal_share(user, b"share").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let again = load_or_create(&path, &counting(3)).await.unwrap();
        assert_eq!(
            again
                .open_share(user, &sealed)
                .as_deref()
                .map(Vec::as_slice),
            Some(&b"share"[..])
        );
        assert_eq!(
            server_setup_bytes(again.setup()).as_slice(),
            server_setup_bytes(key.setup()).as_slice()
        );
    }

    #[tokio::test]
    async fn a_missing_key_file_with_stored_passwords_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("password-key.json");
        let err = load_or_create(&path, &counting(2)).await.unwrap_err();
        assert!(format!("{err:#}").contains("restore the file"), "{err:#}");
        assert!(!path.exists());
    }

    #[test]
    fn a_damaged_or_open_key_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("password-key.json");
        write_secret(
            &path,
            "{\"v\":1,\"opaqueSetup\":\"short\",\"shareKey\":\"short\",\
             \"createdAt\":\"2026-10-24T00:00:00Z\"}",
        )
        .unwrap();
        assert!(read_key_file(&path).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let good = dir.path().join("open.json");
            write_secret(
                &good,
                &key_file_text(&PasswordKey::generate().unwrap(), Utc::now()),
            )
            .unwrap();
            assert!(read_key_file(&good).is_ok());
            fs::set_permissions(&good, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read_key_file(&good).is_err());
        }
    }
}
