//! This server's key: the key file, the key log in the database, the descriptor, and the
//! operations that change them (first boot, rotation, recovery).
//!
//! The server key's seed lives only in the key file (`{"v":1,"seed":"<base64url>"}`, mode
//! 0600); the database holds the signed log. A new seed is written next to the key file before
//! its log entry commits and moved over the key file after, so a crash in between leaves a file
//! the next start recognises.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use rim_core::key_identity::model::KeyLogEntryType;
use rim_core::s2s::model::{ServerDescriptorRecord, ServerKeyLogRecord, SignedRecord};
use rim_core::s2s::repository::ServerIdentityRepository;
use rim_identity::SignedStatement;
use rim_s2s::{
    Endpoints, Limits, MAX_LOG_ENTRIES, Registration, Role, ServerDescriptor, ServerIdentity,
    ServerKeyState, ServerRotationSigner, ServerSigner, SharedIdentity, sign_descriptor,
    sign_inception, sign_rotation, timestamp, verify_descriptor,
};
use serde::{Deserialize, Serialize};

use crate::secret_file::{check_private, write_secret};

/// Prefix of the recovery key file's single line.
const RECOVERY_PREFIX: &str = "rim-server-recovery:1:";

/// Version of the key file format.
const KEY_FILE_VERSION: u32 = 1;

/// Where the server key and the first recovery key are kept on disk.
#[derive(Debug, Clone)]
pub struct KeyFiles {
    pub key_file: PathBuf,
    pub data_dir: PathBuf,
}

impl KeyFiles {
    /// `key_file`, or `<data_dir>/server-key.json` when unset.
    pub fn new(data_dir: impl Into<PathBuf>, key_file: Option<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        Self {
            key_file: key_file.unwrap_or_else(|| data_dir.join("server-key.json")),
            data_dir,
        }
    }

    /// Where first boot writes the recovery key: `<data_dir>/server-recovery-key.txt`.
    pub fn recovery_file(&self) -> PathBuf {
        self.data_dir.join("server-recovery-key.txt")
    }

    /// Where a new seed waits until its log entry commits.
    fn next_file(&self) -> PathBuf {
        let mut name = self.key_file.as_os_str().to_owned();
        name.push(".next");
        PathBuf::from(name)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyFile {
    v: u32,
    seed: String,
}

/// The head server key with the verified log it heads.
#[derive(Debug)]
pub struct LoadedKey {
    pub signer: ServerSigner,
    pub state: ServerKeyState,
    pub log: Vec<SignedStatement>,
}

/// What the descriptor says besides the key: roles, endpoints, limits and registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorSettings {
    pub roles: Vec<Role>,
    pub endpoints: Endpoints,
    pub limits: Limits,
    pub registration: Registration,
    pub mls_cipher_suites: Vec<i32>,
}

fn to_statement(record: &SignedRecord) -> SignedStatement {
    SignedStatement {
        payload: record.payload.clone(),
        signature: record.signature.clone(),
    }
}

fn to_record(statement: &SignedStatement) -> SignedRecord {
    SignedRecord {
        payload: statement.payload.clone(),
        signature: statement.signature.clone(),
    }
}

fn key_file_text(signer: &ServerSigner) -> String {
    let file = KeyFile {
        v: KEY_FILE_VERSION,
        seed: URL_SAFE_NO_PAD.encode(signer.seed()),
    };
    serde_json::to_string(&file).unwrap_or_default()
}

fn decode_seed(text: &str) -> anyhow::Result<ServerSigner> {
    let bytes = URL_SAFE_NO_PAD
        .decode(text.trim())
        .map_err(|_| anyhow!("the seed is not unpadded base64url"))?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow!("the seed is not 32 bytes"))?;
    Ok(ServerSigner::from_seed(seed))
}

fn read_key_file(path: &Path) -> anyhow::Result<ServerSigner> {
    check_private(path)?;
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let file: KeyFile = serde_json::from_str(&text)
        .with_context(|| format!("{} is not a server key file", path.display()))?;
    if file.v != KEY_FILE_VERSION {
        bail!("{} has unsupported version {}", path.display(), file.v);
    }
    decode_seed(&file.seed).with_context(|| format!("{} holds no valid seed", path.display()))
}

/// Read a recovery key file: one line `rim-server-recovery:1:<base64url seed>`.
pub fn read_recovery_file(path: &Path) -> anyhow::Result<ServerSigner> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let seed = text
        .trim()
        .strip_prefix(RECOVERY_PREFIX)
        .ok_or_else(|| anyhow!("{} is not a server recovery key file", path.display()))?;
    decode_seed(seed)
}

fn recovery_file_text(signer: &ServerSigner) -> String {
    format!(
        "{RECOVERY_PREFIX}{}\n",
        URL_SAFE_NO_PAD.encode(signer.seed())
    )
}

/// Warn when the first-boot recovery key is still next to the server key.
fn warn_about_recovery_file(files: &KeyFiles) {
    let path = files.recovery_file();
    if path.exists() {
        tracing::warn!(
            path = %path.display(),
            "move the server recovery key offline: anyone who can read it can replace this \
             server's key"
        );
    }
}

async fn read_log(repo: &dyn ServerIdentityRepository) -> anyhow::Result<Vec<SignedStatement>> {
    let records = repo
        .key_log()
        .await
        .map_err(|e| anyhow!("cannot read the server key log: {e}"))?;
    for (i, record) in records.iter().enumerate() {
        if usize::try_from(record.seq).ok() != Some(i) {
            bail!("the server key log has a gap at seq {i}");
        }
    }
    Ok(records.iter().map(|r| to_statement(&r.statement)).collect())
}

/// The key file's signer, or the waiting next seed when that is the one the log head names
/// (a rotation that committed before its seed was moved into place).
fn signer_for_head(files: &KeyFiles, state: &ServerKeyState) -> anyhow::Result<ServerSigner> {
    let next = files.next_file();
    if files.key_file.exists() {
        let signer = read_key_file(&files.key_file)?;
        if signer.public_key() == state.server_key {
            return Ok(signer);
        }
    }
    if next.exists() {
        let signer = read_key_file(&next)?;
        if signer.public_key() == state.server_key {
            fs::rename(&next, &files.key_file)
                .with_context(|| format!("cannot move {} into place", next.display()))?;
            tracing::info!(
                path = %files.key_file.display(),
                "moved the committed rotation's server key into place"
            );
            return Ok(signer);
        }
    }
    if files.key_file.exists() {
        bail!(
            "{} does not hold the server key the key log's head (seq {}) names; restore the \
             right file or run `rim_server server-key recover`",
            files.key_file.display(),
            state.seq
        );
    }
    bail!(
        "the server key log exists but {} is missing; restore it or run `rim_server server-key \
         recover`",
        files.key_file.display()
    )
}

/// Load the server key and its log. Fails when there is no key yet.
pub async fn load(
    repo: &dyn ServerIdentityRepository,
    files: &KeyFiles,
) -> anyhow::Result<LoadedKey> {
    let log = read_log(repo).await?;
    if log.is_empty() {
        bail!("this server has no key yet; start the server once to create it");
    }
    let state = ServerKeyState::replay(&log).context("the server key log does not verify")?;
    let signer = signer_for_head(files, &state)?;
    warn_about_recovery_file(files);
    Ok(LoadedKey { signer, state, log })
}

/// Load the server key and its log, creating both on first boot: with neither a key file nor a
/// log, generate the server and recovery keys, write the key file and the recovery key file
/// (both 0600), and store the inception. Refuse to start when only one of them exists, when
/// they disagree, or when the key file is readable by group or others.
pub async fn load_or_create(
    repo: &dyn ServerIdentityRepository,
    files: &KeyFiles,
) -> anyhow::Result<LoadedKey> {
    let log = read_log(repo).await?;
    if !log.is_empty() {
        return load(repo, files).await;
    }
    if files.key_file.exists() {
        bail!(
            "{} exists but the server key log is empty; restore the database or move the file \
             away to create a new server identity",
            files.key_file.display()
        );
    }
    fs::create_dir_all(&files.data_dir)
        .with_context(|| format!("cannot create {}", files.data_dir.display()))?;
    let recovery_path = files.recovery_file();
    if recovery_path.exists() {
        bail!(
            "{} exists but there is no server key; move it away before the first start",
            recovery_path.display()
        );
    }

    let signer = ServerSigner::generate().map_err(|e| anyhow!("entropy failure: {e}"))?;
    let recovery = ServerSigner::generate().map_err(|e| anyhow!("entropy failure: {e}"))?;
    let inception = sign_inception(&signer, &recovery, Utc::now());
    let state = ServerKeyState::from_inception(&inception)?;

    write_secret(&recovery_path, &recovery_file_text(&recovery))?;
    if let Err(e) = write_secret(&files.key_file, &key_file_text(&signer)) {
        let _ = fs::remove_file(&recovery_path);
        return Err(e);
    }
    let stored = repo
        .append_key_log_entry(ServerKeyLogRecord {
            seq: 0,
            entry_type: KeyLogEntryType::Inception,
            statement: to_record(&inception),
        })
        .await;
    if let Err(e) = stored {
        let _ = fs::remove_file(&files.key_file);
        let _ = fs::remove_file(&recovery_path);
        bail!("cannot store the server key log's inception: {e}");
    }
    tracing::info!(
        target: "audit",
        event = "server_key.created",
        server_id = %state.server_id,
        "created this server's key and key log"
    );
    warn_about_recovery_file(files);
    Ok(LoadedKey {
        signer,
        state,
        log: vec![inception],
    })
}

/// Append `entry`, which makes `next_signer` the head key: write the new seed next to the key
/// file, commit the entry, then move the seed into place.
async fn commit_rotation(
    repo: &dyn ServerIdentityRepository,
    files: &KeyFiles,
    mut log: Vec<SignedStatement>,
    state: &ServerKeyState,
    entry: SignedStatement,
    next_signer: ServerSigner,
) -> anyhow::Result<LoadedKey> {
    if log.len() >= MAX_LOG_ENTRIES {
        bail!("the server key log already has {MAX_LOG_ENTRIES} entries");
    }
    let next_state = state.apply(&entry)?;
    let next_path = files.next_file();
    if next_path.exists() {
        fs::remove_file(&next_path)
            .with_context(|| format!("cannot remove {}", next_path.display()))?;
    }
    write_secret(&next_path, &key_file_text(&next_signer))?;
    let stored = repo
        .append_key_log_entry(ServerKeyLogRecord {
            seq: next_state.seq,
            entry_type: KeyLogEntryType::Rotate,
            statement: to_record(&entry),
        })
        .await;
    if let Err(e) = stored {
        let _ = fs::remove_file(&next_path);
        bail!("cannot store the rotation (did another process rotate first?): {e}");
    }
    fs::rename(&next_path, &files.key_file).with_context(|| {
        format!(
            "the rotation committed but {} could not be moved over {}; move it by hand",
            next_path.display(),
            files.key_file.display()
        )
    })?;
    log.push(entry);
    Ok(LoadedKey {
        signer: next_signer,
        state: next_state,
        log,
    })
}

/// Rotate to a new server key with a `signer: server` entry. Peers accept the previous key for
/// 24 hours after they learn of the rotation.
pub async fn rotate(
    repo: &dyn ServerIdentityRepository,
    files: &KeyFiles,
) -> anyhow::Result<LoadedKey> {
    let current = load(repo, files).await?;
    let next = ServerSigner::generate().map_err(|e| anyhow!("entropy failure: {e}"))?;
    let entry = sign_rotation(
        &current.state,
        &current.signer,
        ServerRotationSigner::Server,
        &next.public_key(),
        &current.state.recovery_key,
        Utc::now(),
    );
    let loaded = commit_rotation(repo, files, current.log, &current.state, entry, next).await?;
    tracing::info!(
        target: "audit",
        event = "server_key.rotated",
        server_id = %loaded.state.server_id,
        seq = loaded.state.seq,
        "rotated the server key"
    );
    Ok(loaded)
}

/// Replace the server key with a `signer: recovery` entry signed by the recovery key in
/// `recovery_key_file`; peers stop accepting the previous key at once. With
/// `new_recovery_key`, the recovery key changes too and the new one is written to the
/// recovery key file in the data directory, which must not exist. Works without the key file.
pub async fn recover(
    repo: &dyn ServerIdentityRepository,
    files: &KeyFiles,
    recovery_key_file: &Path,
    new_recovery_key: bool,
) -> anyhow::Result<(LoadedKey, Option<PathBuf>)> {
    let log = read_log(repo).await?;
    if log.is_empty() {
        bail!("this server has no key log to recover");
    }
    let state = ServerKeyState::replay(&log).context("the server key log does not verify")?;
    let recovery = read_recovery_file(recovery_key_file)?;
    if recovery.public_key() != state.recovery_key {
        bail!(
            "{} does not hold this server's current recovery key",
            recovery_key_file.display()
        );
    }
    let next = ServerSigner::generate().map_err(|e| anyhow!("entropy failure: {e}"))?;
    let (next_recovery, written) = if new_recovery_key {
        let fresh = ServerSigner::generate().map_err(|e| anyhow!("entropy failure: {e}"))?;
        let path = files.recovery_file();
        fs::create_dir_all(&files.data_dir)
            .with_context(|| format!("cannot create {}", files.data_dir.display()))?;
        write_secret(&path, &recovery_file_text(&fresh))?;
        (fresh.public_key(), Some(path))
    } else {
        (recovery.public_key(), None)
    };
    let entry = sign_rotation(
        &state,
        &recovery,
        ServerRotationSigner::Recovery,
        &next.public_key(),
        &next_recovery,
        Utc::now(),
    );
    let loaded = match commit_rotation(repo, files, log, &state, entry, next).await {
        Ok(loaded) => loaded,
        Err(e) => {
            if let Some(path) = &written {
                let _ = fs::remove_file(path);
            }
            return Err(e);
        }
    };
    tracing::info!(
        target: "audit",
        event = "server_key.recovered",
        server_id = %loaded.state.server_id,
        seq = loaded.state.seq,
        recovery_key_changed = new_recovery_key,
        "replaced the server key with the recovery key"
    );
    Ok((loaded, written))
}

/// Build the identity this server publishes: reuse the stored descriptor when it verifies
/// against the head key and says what `settings` say; otherwise sign one with `version + 1`
/// and store it.
pub async fn with_descriptor(
    repo: &dyn ServerIdentityRepository,
    loaded: LoadedKey,
    settings: &DescriptorSettings,
) -> anyhow::Result<ServerIdentity> {
    let desired = |version: u32| {
        ServerDescriptor::new(
            loaded.state.server_id.clone(),
            loaded.state.seq,
            version,
            settings.roles.clone(),
            settings.endpoints.clone(),
            settings.mls_cipher_suites.clone(),
            settings.limits.clone(),
            settings.registration.clone(),
            timestamp(Utc::now()),
        )
    };
    for _ in 0..2 {
        let stored = repo
            .descriptor()
            .await
            .map_err(|e| anyhow!("cannot read the server descriptor: {e}"))?;
        let mut version = 1;
        if let Some(stored) = &stored {
            let statement = to_statement(&stored.statement);
            if let Ok(content) = verify_descriptor(&loaded.state, &statement)
                && content.same_content(&desired(content.version))
            {
                return Ok(ServerIdentity {
                    signer: loaded.signer,
                    state: loaded.state,
                    log: loaded.log,
                    descriptor: statement,
                    descriptor_content: content,
                });
            }
            version = stored.version.saturating_add(1);
        }
        let content = desired(version);
        let statement = sign_descriptor(&loaded.signer, &content)?;
        let stored = repo
            .store_descriptor(ServerDescriptorRecord {
                version,
                statement: to_record(&statement),
            })
            .await
            .map_err(|e| anyhow!("cannot store the server descriptor: {e}"))?;
        if stored {
            tracing::info!(
                server_id = %loaded.state.server_id,
                key_seq = loaded.state.seq,
                version,
                "signed a new server descriptor"
            );
            return Ok(ServerIdentity {
                signer: loaded.signer,
                state: loaded.state,
                log: loaded.log,
                descriptor: statement,
                descriptor_content: content,
            });
        }
    }
    bail!(
        "another node keeps replacing the server descriptor; check that every node has the same configuration"
    )
}

/// Pick up a rotation made by `rim_server server-key` while this process runs: when the
/// stored log's head differs from `shared`'s, load the key file again, re-sign the descriptor
/// and replace the shared identity. Returns whether it did.
pub async fn reload_if_rotated(
    repo: &dyn ServerIdentityRepository,
    files: &KeyFiles,
    shared: &SharedIdentity,
    settings: &DescriptorSettings,
) -> anyhow::Result<bool> {
    let current = shared.current();
    let log = read_log(repo).await?;
    let unchanged = log.len() == current.log.len()
        && log
            .last()
            .zip(current.log.last())
            .is_some_and(|(a, b)| a.payload == b.payload);
    if unchanged {
        return Ok(false);
    }
    let loaded = load(repo, files).await?;
    let identity = with_descriptor(repo, loaded, settings).await?;
    tracing::info!(
        server_id = %identity.state.server_id,
        key_seq = identity.state.seq,
        "now signing with the rotated server key"
    );
    shared.replace(identity);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
    use rim_s2s::{RegistrationMode, key_hash};

    use super::*;

    /// An in-memory key log and descriptor.
    #[derive(Default)]
    struct MemoryRepo {
        log: Mutex<Vec<ServerKeyLogRecord>>,
        descriptor: Mutex<Option<ServerDescriptorRecord>>,
    }

    #[async_trait]
    impl ServerIdentityRepository for MemoryRepo {
        async fn key_log(&self) -> RepoResult<Vec<ServerKeyLogRecord>> {
            Ok(self.log.lock().expect("lock").clone())
        }

        async fn append_key_log_entry(&self, entry: ServerKeyLogRecord) -> RepoResult<()> {
            let mut log = self.log.lock().expect("lock");
            if log.iter().any(|e| e.seq == entry.seq) {
                return Err(RepoError::new(RepoErrorKind::Conflict));
            }
            log.push(entry);
            Ok(())
        }

        async fn descriptor(&self) -> RepoResult<Option<ServerDescriptorRecord>> {
            Ok(self.descriptor.lock().expect("lock").clone())
        }

        async fn store_descriptor(&self, d: ServerDescriptorRecord) -> RepoResult<bool> {
            let mut stored = self.descriptor.lock().expect("lock");
            if stored.as_ref().is_some_and(|s| s.version >= d.version) {
                return Ok(false);
            }
            *stored = Some(d);
            Ok(true)
        }
    }

    fn files(dir: &Path) -> KeyFiles {
        KeyFiles::new(dir, None)
    }

    fn settings(s2s: Option<&str>) -> DescriptorSettings {
        DescriptorSettings {
            roles: vec![Role::Host, Role::Relay],
            endpoints: Endpoints {
                graphql: "https://a.example/api".into(),
                websocket: "wss://a.example/api/ws".into(),
                s2s: s2s.map(str::to_string),
            },
            limits: Limits {
                max_body_bytes: 8 * 1024 * 1024,
                clock_skew_seconds: 300,
                max_deliver_items: 256,
            },
            registration: Registration {
                mode: RegistrationMode::Invite,
                pow_bits: None,
            },
            mls_cipher_suites: vec![79],
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).expect("meta").permissions().mode() & 0o777
    }

    #[tokio::test]
    async fn first_boot_creates_both_keys_and_the_inception() {
        let dir = tempfile::tempdir().expect("dir");
        let repo = MemoryRepo::default();
        let files = files(dir.path());
        let loaded = load_or_create(&repo, &files).await.expect("create");
        assert_eq!(loaded.state.seq, 0);
        assert_eq!(repo.log.lock().expect("lock").len(), 1);
        assert!(files.key_file.exists());
        let recovery = read_recovery_file(&files.recovery_file()).expect("recovery");
        assert_eq!(recovery.public_key(), loaded.state.recovery_key);
        #[cfg(unix)]
        {
            assert_eq!(mode(&files.key_file), 0o600);
            assert_eq!(mode(&files.recovery_file()), 0o600);
        }
        // The second start loads the same identity.
        let again = load_or_create(&repo, &files).await.expect("load");
        assert_eq!(again.state, loaded.state);
    }

    #[tokio::test]
    async fn start_is_refused_on_a_missing_or_mismatched_file_or_log() {
        let dir = tempfile::tempdir().expect("dir");
        let files = files(dir.path());
        let repo = MemoryRepo::default();
        load_or_create(&repo, &files).await.expect("create");

        // File without a log.
        let empty = MemoryRepo::default();
        assert!(load_or_create(&empty, &files).await.is_err());

        // Log without the file.
        let saved = fs::read(&files.key_file).expect("read");
        fs::remove_file(&files.key_file).expect("rm");
        assert!(load_or_create(&repo, &files).await.is_err());

        // A file with another key.
        write_secret(
            &files.key_file,
            &key_file_text(&ServerSigner::from_seed([1; 32])),
        )
        .expect("write");
        assert!(load_or_create(&repo, &files).await.is_err());
        fs::remove_file(&files.key_file).expect("rm");
        fs::write(&files.key_file, saved).expect("restore");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&files.key_file, fs::Permissions::from_mode(0o644)).expect("chmod");
            let err = load_or_create(&repo, &files)
                .await
                .expect_err("world readable");
            assert!(
                err.to_string().contains("readable by group or others"),
                "{err}"
            );
            fs::set_permissions(&files.key_file, fs::Permissions::from_mode(0o600)).expect("chmod");
        }
        load_or_create(&repo, &files).await.expect("restored");
    }

    #[tokio::test]
    async fn rotation_replaces_the_key_file_and_extends_the_log() {
        let dir = tempfile::tempdir().expect("dir");
        let files = files(dir.path());
        let repo = MemoryRepo::default();
        let first = load_or_create(&repo, &files).await.expect("create");
        let rotated = rotate(&repo, &files).await.expect("rotate");
        assert_eq!(rotated.state.seq, 1);
        assert_eq!(
            rotated.state.head_signer(),
            Some(ServerRotationSigner::Server)
        );
        assert_ne!(rotated.state.server_key, first.state.server_key);
        assert_eq!(rotated.state.server_id, first.state.server_id);
        let loaded = load(&repo, &files).await.expect("load");
        assert_eq!(loaded.signer.public_key(), rotated.state.server_key);
        assert!(!files.next_file().exists());
    }

    #[tokio::test]
    async fn a_committed_rotation_whose_seed_was_not_moved_is_picked_up() {
        let dir = tempfile::tempdir().expect("dir");
        let files = files(dir.path());
        let repo = MemoryRepo::default();
        let first = load_or_create(&repo, &files).await.expect("create");
        let next = ServerSigner::generate().expect("key");
        let entry = sign_rotation(
            &first.state,
            &first.signer,
            ServerRotationSigner::Server,
            &next.public_key(),
            &first.state.recovery_key,
            Utc::now(),
        );
        write_secret(&files.next_file(), &key_file_text(&next)).expect("next");
        repo.append_key_log_entry(ServerKeyLogRecord {
            seq: 1,
            entry_type: KeyLogEntryType::Rotate,
            statement: to_record(&entry),
        })
        .await
        .expect("append");
        let loaded = load(&repo, &files).await.expect("load");
        assert_eq!(loaded.signer.public_key(), next.public_key());
        assert!(!files.next_file().exists());
    }

    #[tokio::test]
    async fn recovery_needs_the_recovery_key_and_may_replace_it() {
        let dir = tempfile::tempdir().expect("dir");
        let files = files(dir.path());
        let repo = MemoryRepo::default();
        let first = load_or_create(&repo, &files).await.expect("create");
        let offline = dir.path().join("offline.txt");
        fs::rename(files.recovery_file(), &offline).expect("move offline");
        fs::remove_file(&files.key_file).expect("lose the server key");

        let wrong = dir.path().join("wrong.txt");
        fs::write(
            &wrong,
            recovery_file_text(&ServerSigner::from_seed([3; 32])),
        )
        .expect("write");
        assert!(recover(&repo, &files, &wrong, false).await.is_err());

        let (recovered, written) = recover(&repo, &files, &offline, true)
            .await
            .expect("recover");
        assert_eq!(
            recovered.state.head_signer(),
            Some(ServerRotationSigner::Recovery)
        );
        assert_ne!(recovered.state.recovery_key, first.state.recovery_key);
        let written = written.expect("new recovery file");
        assert_eq!(
            read_recovery_file(&written).expect("read").public_key(),
            recovered.state.recovery_key
        );
        let loaded = load(&repo, &files).await.expect("load");
        assert_eq!(
            key_hash(&loaded.state.server_key),
            key_hash(&recovered.state.server_key)
        );
    }

    #[tokio::test]
    async fn descriptor_is_reused_until_settings_or_the_key_change() {
        let dir = tempfile::tempdir().expect("dir");
        let files = files(dir.path());
        let repo = MemoryRepo::default();
        let loaded = load_or_create(&repo, &files).await.expect("create");
        let identity = with_descriptor(&repo, loaded, &settings(None))
            .await
            .expect("descriptor");
        assert_eq!(identity.descriptor_content.version, 1);

        let loaded = load(&repo, &files).await.expect("load");
        let same = with_descriptor(&repo, loaded, &settings(None))
            .await
            .expect("descriptor");
        assert_eq!(same.descriptor, identity.descriptor);

        let loaded = load(&repo, &files).await.expect("load");
        let changed = with_descriptor(&repo, loaded, &settings(Some("https://a.example/s2s/v1")))
            .await
            .expect("descriptor");
        assert_eq!(changed.descriptor_content.version, 2);

        let shared = SharedIdentity::new(changed);
        assert!(
            !reload_if_rotated(
                &repo,
                &files,
                &shared,
                &settings(Some("https://a.example/s2s/v1"))
            )
            .await
            .expect("reload")
        );
        rotate(&repo, &files).await.expect("rotate");
        assert!(
            reload_if_rotated(
                &repo,
                &files,
                &shared,
                &settings(Some("https://a.example/s2s/v1"))
            )
            .await
            .expect("reload")
        );
        let current = shared.current();
        assert_eq!(current.state.seq, 1);
        assert_eq!(current.descriptor_content.key_seq, 1);
        assert_eq!(current.descriptor_content.version, 3);
    }
}
