use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use uuid::Uuid;

use crate::key_identity::model::{
    DeviceKey, DeviceKeyId, KeyChallenge, KeyChallengeId, KeyLogEntry, NewDeviceKey,
    NewKeyLogEntry, StoredProfileCard,
};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

/// Key logs, device keys and profile cards of the users on this server.
///
/// Writes that depend on the current key log (enroll, rotate, add or revoke a device, publish a
/// card) call [`KeyIdentityRepository::lock_user`] first, inside one transaction, so two of them
/// for the same user never interleave.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait KeyIdentityRepository: Send + Sync {
    /// Lock the user's row until the surrounding transaction ends (`SELECT ... FOR UPDATE`).
    /// Returns `false` when the user does not exist.
    async fn lock_user(&self, user_id: UserId) -> RepoResult<bool>;

    /// The user who enrolled `principal_id` on this server.
    async fn find_user_by_principal_id(&self, principal_id: &str) -> RepoResult<Option<UserId>>;

    /// The user's key log, oldest entry first. Empty when the user has no key.
    async fn list_key_log(&self, user_id: UserId) -> RepoResult<Vec<KeyLogEntry>>;

    /// Bind a key identity to a user that has none: set `principal_id` and `identity_key` and
    /// store the log entries.
    ///
    /// # Errors
    /// `Conflict` when the user already has a principal id, or another user has this one.
    async fn enroll(
        &self,
        user_id: UserId,
        principal_id: &str,
        identity_key: &str,
        entries: Vec<NewKeyLogEntry>,
    ) -> RepoResult<()>;

    /// Store the key log of a user created with its key identity already set (key-only
    /// signup, through `NewUser::key_identity`), in the same transaction as the user row.
    ///
    /// # Errors
    /// `Conflict` when the user already has log entries.
    async fn insert_key_log(&self, user_id: UserId, entries: Vec<NewKeyLogEntry>)
    -> RepoResult<()>;

    /// Append a rotation and make `identity_key` current.
    ///
    /// # Errors
    /// `Conflict` when an entry with this seq already exists.
    async fn append_rotation(
        &self,
        user_id: UserId,
        entry: NewKeyLogEntry,
        identity_key: &str,
    ) -> RepoResult<()>;

    /// # Errors
    /// `Conflict` when the user already has a device with this `device_id` or public key,
    /// revoked or not.
    async fn add_device(&self, device: NewDeviceKey) -> RepoResult<DeviceKey>;

    async fn get_device(&self, user_id: UserId, device_id: Uuid) -> RepoResult<Option<DeviceKey>>;

    /// Every device of the user, revoked ones included, oldest first.
    async fn list_devices(&self, user_id: UserId) -> RepoResult<Vec<DeviceKey>>;

    /// Mark a device revoked, keeping the signed revocation. Returns `false` when it already
    /// was revoked (the first revocation is kept).
    async fn revoke_device(
        &self,
        id: DeviceKeyId,
        revocation_payload: &str,
        revocation_signature: &str,
    ) -> RepoResult<bool>;

    /// Mark every unrevoked device of the user revoked. Returns how many were.
    async fn revoke_all_devices(&self, user_id: UserId) -> RepoResult<u64>;

    /// Record that the device just signed in.
    async fn touch_device(&self, id: DeviceKeyId) -> RepoResult<()>;

    async fn get_profile_card(&self, user_id: UserId) -> RepoResult<Option<StoredProfileCard>>;

    /// Store a card, replacing the stored one only if `version` is greater. Returns `false`
    /// when the stored card's version is the same or greater.
    async fn upsert_profile_card(
        &self,
        user_id: UserId,
        version: u32,
        payload: &str,
        signature: &str,
    ) -> RepoResult<bool>;
}

/// Single-use key login challenges.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait KeyChallengeRepository: Send + Sync {
    async fn create(&self, nonce: &str, expires_at: DateTime<Utc>) -> RepoResult<KeyChallenge>;

    /// Atomically mark the challenge used if it exists, carries `nonce`, is unused and has not
    /// expired. Returns `true` only for the one caller that claimed it.
    async fn consume(&self, id: KeyChallengeId, nonce: &str) -> RepoResult<bool>;

    /// Delete challenges that expired before `before`. Returns how many were deleted.
    async fn delete_expired(&self, before: DateTime<Utc>) -> RepoResult<u64>;
}
