//! The server's franking keys: the key file that holds them, the key that signs new receipts and
//! the older keys that still verify the receipts they signed.
//!
//! The key file is `{"v":1,"keys":[{"id":1,"key":"<base64url, 32 bytes>","createdAt":"…"}]}`,
//! mode 0600. The key with the highest id signs; a key a newer one replaced keeps verifying for
//! [`RETIRED_KEY_WINDOW`] after its replacement was created, and the next rotation after that
//! drops it from the file. A rotation writes `<file>.next` and moves it over the file.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use anyhow::{Context, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use rim_core::e2ee::repository::E2eeRepository;

use crate::secret_file::{check_private, write_secret};

/// How long a replaced key keeps verifying receipts, counted from its replacement's creation.
pub const RETIRED_KEY_WINDOW: Duration = Duration::days(30);

/// Version of the key file format.
const KEY_FILE_VERSION: u32 = 1;

/// Length of a franking key.
const KEY_LEN: usize = 32;

/// Least time between two reads of the key file that a report naming an unknown key causes.
const UNKNOWN_KEY_RELOAD_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyFile {
    v: u32,
    keys: Vec<KeyFileEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct KeyFileEntry {
    id: i32,
    key: String,
    created_at: DateTime<Utc>,
}

impl Drop for KeyFileEntry {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

/// One franking key.
pub struct FrankingKey {
    pub id: i32,
    secret: Zeroizing<[u8; KEY_LEN]>,
    pub created_at: DateTime<Utc>,
}

impl FrankingKey {
    pub fn new(id: i32, secret: [u8; KEY_LEN], created_at: DateTime<Utc>) -> Self {
        Self {
            id,
            secret: Zeroizing::new(secret),
            created_at,
        }
    }

    /// A new key from the OS random generator.
    fn generate(id: i32, created_at: DateTime<Utc>) -> anyhow::Result<Self> {
        let mut secret = Zeroizing::new([0u8; KEY_LEN]);
        getrandom::fill(secret.as_mut()).map_err(|e| anyhow!("entropy failure: {e}"))?;
        Ok(Self {
            id,
            secret,
            created_at,
        })
    }

    pub fn secret(&self) -> &[u8; KEY_LEN] {
        &self.secret
    }
}

/// The secret is wiped from memory when the key is dropped.
impl ZeroizeOnDrop for FrankingKey {}

impl std::fmt::Debug for FrankingKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrankingKey")
            .field("id", &self.id)
            .field("created_at", &self.created_at)
            .finish_non_exhaustive()
    }
}

/// The keys of a key file, ordered by id; never empty.
#[derive(Clone, Debug)]
pub struct FrankingKeys(Arc<Vec<FrankingKey>>);

impl FrankingKeys {
    /// The keys, which must be at least one with distinct positive ids.
    pub fn new(mut keys: Vec<FrankingKey>) -> anyhow::Result<Self> {
        keys.sort_by_key(|k| k.id);
        if keys.is_empty() {
            bail!("there is no franking key");
        }
        if keys.iter().any(|k| k.id < 1) {
            bail!("franking key ids start at 1");
        }
        if keys.windows(2).any(|w| w[0].id == w[1].id) {
            bail!("two franking keys share an id");
        }
        Ok(Self(Arc::new(keys)))
    }

    /// The key that signs new receipts: the one with the highest id.
    pub fn current(&self) -> &FrankingKey {
        // `new` refuses an empty list.
        self.0
            .last()
            .unwrap_or_else(|| unreachable!("a key set is never empty"))
    }

    /// Key `id` when it may verify a receipt at `now`: the current key, or an older one whose
    /// replacement was created less than [`RETIRED_KEY_WINDOW`] ago.
    pub fn verifying(&self, id: i32, now: DateTime<Utc>) -> Option<&FrankingKey> {
        let index = self.0.iter().position(|k| k.id == id)?;
        match self.0.get(index + 1) {
            None => Some(&self.0[index]),
            Some(next) if now < next.created_at + RETIRED_KEY_WINDOW => Some(&self.0[index]),
            Some(_) => None,
        }
    }

    /// Every key, oldest first.
    pub fn keys(&self) -> &[FrankingKey] {
        &self.0
    }

    /// The ids, oldest first.
    fn ids(&self) -> Vec<i32> {
        self.0.iter().map(|k| k.id).collect()
    }
}

/// The key set the services sign and verify with. It follows the key file, so a rotation made
/// by the CLI reaches a running server.
#[derive(Clone, Debug)]
pub struct SharedFrankingKeys {
    keys: Arc<RwLock<FrankingKeys>>,
    file: Option<PathBuf>,
    /// When a report naming an unknown key last made this node read the file.
    unknown_key_reload: Arc<Mutex<Option<Instant>>>,
}

impl SharedFrankingKeys {
    /// Keys that never change; for tests.
    pub fn fixed(keys: FrankingKeys) -> Self {
        Self {
            keys: Arc::new(RwLock::new(keys)),
            file: None,
            unknown_key_reload: Arc::default(),
        }
    }

    /// Keys read from `file`, read again by [`Self::reload`].
    pub fn from_file(keys: FrankingKeys, file: PathBuf) -> Self {
        Self {
            keys: Arc::new(RwLock::new(keys)),
            file: Some(file),
            unknown_key_reload: Arc::default(),
        }
    }

    /// Whether a report naming an unknown key may read the file now; at most once per
    /// [`UNKNOWN_KEY_RELOAD_INTERVAL`], so such reports cannot make the node read it each time.
    fn may_reload_for_unknown_key(&self) -> bool {
        let mut last = match self.unknown_key_reload.lock() {
            Ok(last) => last,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = Instant::now();
        if last.is_some_and(|at| now.duration_since(at) < UNKNOWN_KEY_RELOAD_INTERVAL) {
            return false;
        }
        *last = Some(now);
        true
    }

    pub fn keys(&self) -> FrankingKeys {
        match self.keys.read() {
            Ok(keys) => keys.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Read the key file again. Returns whether the key ids changed.
    pub fn reload(&self) -> anyhow::Result<bool> {
        let Some(file) = &self.file else {
            return Ok(false);
        };
        let fresh = read_key_file(file)?;
        let changed = fresh.ids() != self.keys().ids();
        if changed {
            match self.keys.write() {
                Ok(mut keys) => *keys = fresh,
                Err(poisoned) => *poisoned.into_inner() = fresh,
            }
        }
        Ok(changed)
    }

    /// Key `id`'s secret when it may verify a receipt at `now`. A key id above every id this
    /// node holds makes it read the key file again first (at most every ten seconds), so a fresh
    /// rotation verifies at once.
    pub fn verifying(&self, id: i32, now: DateTime<Utc>) -> Option<Zeroizing<[u8; KEY_LEN]>> {
        let found =
            |keys: &FrankingKeys| keys.verifying(id, now).map(|k| Zeroizing::new(*k.secret()));
        let keys = self.keys();
        if let Some(secret) = found(&keys) {
            return Some(secret);
        }
        if id <= keys.current().id || !self.may_reload_for_unknown_key() {
            return None;
        }
        match self.reload() {
            Ok(true) => found(&self.keys()),
            Ok(false) => None,
            Err(e) => {
                tracing::error!(error = %format!("{e:#}"), "could not read the franking key file");
                None
            }
        }
    }
}

fn key_file_text(keys: &FrankingKeys) -> Zeroizing<String> {
    let file = KeyFile {
        v: KEY_FILE_VERSION,
        keys: keys
            .keys()
            .iter()
            .map(|k| KeyFileEntry {
                id: k.id,
                key: URL_SAFE_NO_PAD.encode(k.secret()),
                created_at: k.created_at,
            })
            .collect(),
    };
    Zeroizing::new(serde_json::to_string(&file).unwrap_or_default())
}

/// Read and check a key file: mode 0600, the format, and every key 32 bytes.
pub fn read_key_file(path: &Path) -> anyhow::Result<FrankingKeys> {
    check_private(path)?;
    let text = Zeroizing::new(
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?,
    );
    let file: KeyFile = serde_json::from_str(&text)
        .map_err(|_| anyhow!("{} is not a franking key file", path.display()))?;
    if file.v != KEY_FILE_VERSION {
        bail!("{} has unsupported version {}", path.display(), file.v);
    }
    let keys = file
        .keys
        .iter()
        .map(|entry| {
            let bytes = Zeroizing::new(
                URL_SAFE_NO_PAD
                    .decode(entry.key.trim())
                    .map_err(|_| anyhow!("key {} is not unpadded base64url", entry.id))?,
            );
            let secret: [u8; KEY_LEN] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| anyhow!("key {} is not {KEY_LEN} bytes", entry.id))?;
            Ok(FrankingKey::new(entry.id, secret, entry.created_at))
        })
        .collect::<anyhow::Result<Vec<_>>>()
        .with_context(|| format!("{} holds an invalid key", path.display()))?;
    FrankingKeys::new(keys).with_context(|| format!("{} is not usable", path.display()))
}

/// Where a rotation writes the new file before moving it over the key file.
fn next_file(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".next");
    PathBuf::from(name)
}

/// Load the key file, creating it at first boot. A missing file takes the keys of the
/// `franking_key` table when it has any, so receipts signed before the file existed keep
/// verifying; otherwise a new key 1. Refuses to start when messages carry receipts under a key
/// that neither the file nor the table holds (a lost key file).
pub async fn load_or_create(
    path: &Path,
    repo: &dyn E2eeRepository,
) -> anyhow::Result<FrankingKeys> {
    if path.exists() {
        return read_key_file(path);
    }
    let legacy = repo
        .legacy_franking_keys()
        .await
        .map_err(|e| anyhow!("cannot read the franking_key table: {e}"))?;
    let used = repo
        .max_message_franking_key_id()
        .await
        .map_err(|e| anyhow!("cannot read the messages' franking key ids: {e}"))?;
    let held = legacy.iter().map(|k| k.id).max();
    if let Some(used) = used
        && held.is_none_or(|held| used > held)
    {
        bail!(
            "{} is missing but messages carry receipts under franking key {used}; restore the \
             file",
            path.display()
        );
    }
    let keys = if legacy.is_empty() {
        FrankingKeys::new(vec![FrankingKey::generate(1, Utc::now())?])?
    } else {
        let keys = legacy
            .iter()
            .map(|k| {
                let secret: [u8; KEY_LEN] = k
                    .secret
                    .as_slice()
                    .try_into()
                    .map_err(|_| anyhow!("franking key {} is not {KEY_LEN} bytes", k.id))?;
                Ok(FrankingKey::new(k.id, secret, k.created_at))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        FrankingKeys::new(keys)?
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    match write_secret(path, &key_file_text(&keys)) {
        Ok(()) => {}
        // Another node of the same deployment created it first.
        Err(_) if path.exists() => return read_key_file(path),
        Err(e) => return Err(e),
    }
    if legacy.is_empty() {
        tracing::info!(path = %path.display(), "created the franking key file");
    } else {
        tracing::warn!(
            path = %path.display(),
            keys = ?keys.ids(),
            "moved the franking keys from the database into the key file; run \
             `rim_server franking-key rotate` so the key the database holds stops signing"
        );
    }
    Ok(keys)
}

/// Add a new current key to the key file at `now` and drop the keys that stopped verifying.
pub fn rotate(path: &Path, now: DateTime<Utc>) -> anyhow::Result<FrankingKeys> {
    if !path.exists() {
        bail!(
            "{} does not exist; start the server once to create it",
            path.display()
        );
    }
    let old = read_key_file(path)?;
    let next_id = old
        .current()
        .id
        .checked_add(1)
        .ok_or_else(|| anyhow!("franking key ids are exhausted"))?;
    let mut keys: Vec<FrankingKey> = old
        .keys()
        .iter()
        .filter(|k| old.verifying(k.id, now).is_some())
        .map(|k| FrankingKey::new(k.id, *k.secret(), k.created_at))
        .collect();
    keys.push(FrankingKey::generate(next_id, now)?);
    let keys = FrankingKeys::new(keys)?;
    let next = next_file(path);
    if next.exists() {
        fs::remove_file(&next).with_context(|| format!("cannot remove {}", next.display()))?;
    }
    write_secret(&next, &key_file_text(&keys))?;
    fs::rename(&next, path)
        .with_context(|| format!("cannot move {} into place", next.display()))?;
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    use rim_core::e2ee::model::FrankingKey as LegacyKey;
    use rim_core::e2ee::repository::MockE2eeRepository;

    fn at(days: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_790_000_000, 0).unwrap() + Duration::days(days)
    }

    fn key(id: i32, byte: u8, created: i64) -> FrankingKey {
        FrankingKey::new(id, [byte; KEY_LEN], at(created))
    }

    fn repo(legacy: Vec<(i32, u8)>, used: Option<i32>) -> MockE2eeRepository {
        let mut repo = MockE2eeRepository::new();
        repo.expect_legacy_franking_keys().returning(move || {
            let keys = legacy
                .iter()
                .map(|(id, byte)| LegacyKey {
                    id: *id,
                    secret: Zeroizing::new(vec![*byte; KEY_LEN]),
                    created_at: at(0),
                })
                .collect();
            Box::pin(async move { Ok(keys) })
        });
        repo.expect_max_message_franking_key_id()
            .returning(move || Box::pin(async move { Ok(used) }));
        repo
    }

    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn finding_23_franking_keys_are_wiped_on_drop() {
        fn wiped<T: ZeroizeOnDrop>() {}
        wiped::<FrankingKey>();
    }

    #[test]
    fn finding_7_the_newest_key_signs_and_a_replaced_one_verifies_for_30_days() {
        let keys = FrankingKeys::new(vec![key(2, 2, 10), key(1, 1, 0)]).unwrap();
        assert_eq!(keys.current().id, 2);
        assert!(keys.verifying(2, at(1000)).is_some());
        assert_eq!(
            keys.verifying(1, at(39)).map(|k| k.secret()),
            Some(&[1; 32])
        );
        assert!(keys.verifying(1, at(40)).is_none());
        assert!(keys.verifying(3, at(0)).is_none());
        assert!(FrankingKeys::new(vec![]).is_err());
        assert!(FrankingKeys::new(vec![key(1, 1, 0), key(1, 2, 0)]).is_err());
        assert!(FrankingKeys::new(vec![key(0, 1, 0)]).is_err());
    }

    #[tokio::test]
    async fn finding_7_first_boot_creates_a_private_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keys").join("franking-key.json");
        let keys = load_or_create(&path, &repo(vec![], None)).await.unwrap();
        assert_eq!(keys.current().id, 1);
        assert_eq!(mode(&path), 0o600);
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with(r#"{"v":1,"keys":[{"id":1,"key":""#),
            "{text}"
        );
        // The next start reads the same key without touching the database.
        let again = load_or_create(&path, &MockE2eeRepository::new())
            .await
            .unwrap();
        assert_eq!(again.current().secret(), keys.current().secret());
    }

    #[tokio::test]
    async fn finding_7_the_database_key_moves_into_the_file_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("franking-key.json");
        let keys = load_or_create(&path, &repo(vec![(1, 0x42)], Some(1)))
            .await
            .unwrap();
        assert_eq!(keys.current().id, 1);
        assert_eq!(keys.current().secret(), &[0x42; 32]);
        assert_eq!(
            read_key_file(&path).unwrap().current().secret(),
            &[0x42; 32]
        );
    }

    #[tokio::test]
    async fn finding_7_a_lost_key_file_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("franking-key.json");
        let err = load_or_create(&path, &repo(vec![], Some(2)))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("restore the file"), "{err:#}");
        let err = load_or_create(&path, &repo(vec![(1, 1)], Some(3)))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("franking key 3"), "{err:#}");
        assert!(!path.exists());
    }

    #[test]
    fn finding_7_a_readable_or_malformed_key_file_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("franking-key.json");
        let keys = FrankingKeys::new(vec![key(1, 1, 0)]).unwrap();
        write_secret(&path, &key_file_text(&keys)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let err = read_key_file(&path).unwrap_err();
        assert!(format!("{err:#}").contains("chmod 600"), "{err:#}");

        let short = dir.path().join("short.json");
        write_secret(
            &short,
            r#"{"v":1,"keys":[{"id":1,"key":"AAAA","createdAt":"2026-10-09T00:00:00Z"}]}"#,
        )
        .unwrap();
        let err = read_key_file(&short).unwrap_err();
        assert!(format!("{err:#}").contains("not 32 bytes"), "{err:#}");
        let unknown = dir.path().join("unknown.json");
        write_secret(&unknown, r#"{"v":1,"keys":[],"extra":1}"#).unwrap();
        let err = read_key_file(&unknown).unwrap_err();
        assert!(
            format!("{err:#}").contains("not a franking key file"),
            "{err:#}"
        );
    }

    #[test]
    fn finding_7_rotation_adds_a_key_and_drops_the_expired_ones() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("franking-key.json");
        assert!(rotate(&path, at(0)).is_err(), "no file yet");
        let keys = FrankingKeys::new(vec![key(1, 1, 0)]).unwrap();
        write_secret(&path, &key_file_text(&keys)).unwrap();

        let rotated = rotate(&path, at(10)).unwrap();
        assert_eq!(rotated.ids(), vec![1, 2]);
        assert_ne!(rotated.current().secret(), &[1; 32]);
        assert_eq!(mode(&path), 0o600);
        assert!(!next_file(&path).exists());
        // Key 1 still verifies 29 days after key 2 replaced it, so a rotation keeps it.
        assert_eq!(rotate(&path, at(39)).unwrap().ids(), vec![1, 2, 3]);
        // Past its window key 1 goes; key 2 stays while key 3 is young.
        assert_eq!(rotate(&path, at(41)).unwrap().ids(), vec![2, 3, 4]);
        assert_eq!(read_key_file(&path).unwrap().ids(), vec![2, 3, 4]);
    }

    #[test]
    fn finding_7_a_running_server_picks_up_a_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("franking-key.json");
        let keys = FrankingKeys::new(vec![key(1, 1, 0)]).unwrap();
        write_secret(&path, &key_file_text(&keys)).unwrap();
        let shared = SharedFrankingKeys::from_file(keys, path.clone());
        assert!(!shared.reload().unwrap());

        rotate(&path, Utc::now()).unwrap();
        // A receipt under the new key verifies before the periodic reload ran.
        assert!(shared.verifying(2, Utc::now()).is_some());
        assert_eq!(shared.keys().current().id, 2);
        assert!(shared.verifying(1, Utc::now()).is_some());

        // An unknown id reads the file at most once per interval.
        rotate(&path, Utc::now()).unwrap();
        assert!(shared.verifying(9, Utc::now()).is_none());
        assert!(
            shared.verifying(3, Utc::now()).is_none(),
            "the read for key 9 was moments ago"
        );
        assert!(shared.reload().unwrap());
        assert!(shared.verifying(3, Utc::now()).is_some());
    }
}
