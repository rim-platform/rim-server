use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use uuid::Uuid;

use crate::conversation::model::ConversationId;
use crate::e2ee::model::{
    ClaimedKeyPackage, E2eeDevice, FrankingKey, MlsCommit, MlsDesiredDevice, MlsGroup,
    MlsGroupDevice, MlsKeyPackage, MlsKeyPackageId, MlsKeyPackageStatus, MlsPurged,
    MlsReaddRequest, MlsRetention, MlsWelcome, MlsWelcomeId, NewEncryptedMessage, NewMlsCommit,
    NewMlsGroup, NewMlsKeyPackage, NewMlsReaddRequest, NewMlsWelcome,
};
use crate::key_identity::model::DeviceKeyId;
use crate::message::model::{Message, MessageEncryption, MessageId};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

/// How [`E2eeRepository::lock_current_group`] locks the group row until the transaction ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupLock {
    /// `FOR UPDATE`: a Commit. Serializes Commits, and waits for messages in flight.
    Update,
    /// `FOR SHARE`: an application message. Messages run side by side; a Commit waits for them.
    Share,
}

/// Storage of end-to-end encryption: key packages, MLS groups and their devices, Commits,
/// Welcomes, re-add requests, encrypted message envelopes and franking keys.
///
/// A Commit runs in one transaction: [`E2eeRepository::lock_current_group`] with
/// [`GroupLock::Update`], the epoch check, [`E2eeRepository::insert_commit`],
/// [`E2eeRepository::advance_epoch`], then the device, key package and Welcome writes. An
/// application message takes the same row with [`GroupLock::Share`] and checks the epoch in its
/// insert transaction, so every message of an epoch commits before the Commit that ends it.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait E2eeRepository: Send + Sync {
    // ── Devices ──────────────────────────────────────────────────────────

    async fn get_device(&self, device_key_id: DeviceKeyId) -> RepoResult<Option<E2eeDevice>>;

    /// The device `device_id` of the user holding `principal_id`.
    async fn find_device(
        &self,
        principal_id: &str,
        device_id: Uuid,
    ) -> RepoResult<Option<E2eeDevice>>;

    /// Lock the device row until the transaction ends. Returns `false` when it does not exist.
    async fn lock_device(&self, device_key_id: DeviceKeyId) -> RepoResult<bool>;

    /// Record a key package upload, the device's MLS activity signal.
    async fn touch_key_package_upload(&self, device_key_id: DeviceKeyId) -> RepoResult<()>;

    /// The users among `user_ids` who have a principal id and an unrevoked device that uploaded
    /// key packages at or after `active_since`.
    async fn ready_users(
        &self,
        user_ids: &[UserId],
        active_since: DateTime<Utc>,
    ) -> RepoResult<Vec<UserId>>;

    /// The `device_id`s of the user's unrevoked devices that uploaded key packages at or after
    /// `active_since`.
    async fn mls_active_device_ids(
        &self,
        user_id: UserId,
        active_since: DateTime<Utc>,
    ) -> RepoResult<Vec<Uuid>>;

    /// The unrevoked devices of the conversation's current members that uploaded key packages
    /// at or after `active_since`, with whether each has a key package of `cipher_suite` that
    /// can be handed out.
    async fn desired_devices(
        &self,
        conversation_id: ConversationId,
        active_since: DateTime<Utc>,
        cipher_suite: i32,
    ) -> RepoResult<Vec<MlsDesiredDevice>>;

    /// The current members of the conversation holding one of `principal_ids`.
    async fn members_by_principal(
        &self,
        conversation_id: ConversationId,
        principal_ids: &[String],
    ) -> RepoResult<Vec<(String, UserId)>>;

    /// The users holding `principal_ids` who share a workspace with `viewer` (or are the viewer).
    async fn principals_visible_to(
        &self,
        viewer: UserId,
        principal_ids: &[String],
    ) -> RepoResult<Vec<(String, UserId)>>;

    // ── Key packages ─────────────────────────────────────────────────────

    /// Unclaimed one-time key packages of the device and suite that can be handed out.
    async fn count_available_key_packages(
        &self,
        device_key_id: DeviceKeyId,
        cipher_suite: i32,
    ) -> RepoResult<i64>;

    /// Delete the device's last-resort key package of the suite. Returns how many were deleted.
    async fn delete_last_resort_key_package(
        &self,
        device_key_id: DeviceKeyId,
        cipher_suite: i32,
    ) -> RepoResult<u64>;

    /// # Errors
    /// `Conflict` when a key package with this reference exists, or the device already has a
    /// last-resort key package of the suite.
    async fn insert_key_package(
        &self,
        key_package: NewMlsKeyPackage,
    ) -> RepoResult<MlsKeyPackageId>;

    /// The device's key packages, per cipher suite.
    async fn key_package_status(
        &self,
        device_key_id: DeviceKeyId,
    ) -> RepoResult<Vec<MlsKeyPackageStatus>>;

    /// Claim one of the device's unclaimed one-time key packages of the suite that can be
    /// handed out (`FOR UPDATE SKIP LOCKED`), recording who claimed it for which conversation
    /// and incarnation: the oldest, or with `uploaded_after` the newest uploaded after it.
    async fn claim_one_time_key_package(
        &self,
        device_key_id: DeviceKeyId,
        cipher_suite: i32,
        claimed_by: DeviceKeyId,
        conversation_id: ConversationId,
        incarnation: i32,
        uploaded_after: Option<DateTime<Utc>>,
    ) -> RepoResult<Option<ClaimedKeyPackage>>;

    /// Hand out the device's last-resort key package of the suite if it can be handed out. It
    /// stays, records this claim as its latest, and counts it.
    async fn claim_last_resort_key_package(
        &self,
        device_key_id: DeviceKeyId,
        cipher_suite: i32,
        claimed_by: DeviceKeyId,
        conversation_id: ConversationId,
        incarnation: i32,
    ) -> RepoResult<Option<ClaimedKeyPackage>>;

    async fn find_key_packages_by_refs(&self, refs: &[Vec<u8>]) -> RepoResult<Vec<MlsKeyPackage>>;

    /// Delete one-time key packages a Commit used. Last-resort ones are kept.
    async fn consume_key_packages(&self, ids: &[MlsKeyPackageId]) -> RepoResult<u64>;

    // ── Groups ───────────────────────────────────────────────────────────

    /// The conversation's current (unretired) group.
    async fn current_group(&self, conversation_id: ConversationId) -> RepoResult<Option<MlsGroup>>;

    /// Lock the conversation's current group row until the transaction ends.
    async fn lock_current_group(
        &self,
        conversation_id: ConversationId,
        lock: GroupLock,
    ) -> RepoResult<Option<MlsGroup>>;

    /// # Errors
    /// `Conflict` when the incarnation exists, the conversation has a current group, or the
    /// group id is taken.
    async fn insert_group(&self, group: NewMlsGroup) -> RepoResult<MlsGroup>;

    async fn retire_group(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
    ) -> RepoResult<()>;

    /// Move the group to its next epoch and return it.
    async fn advance_epoch(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
    ) -> RepoResult<i64>;

    async fn group_devices(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
    ) -> RepoResult<Vec<MlsGroupDevice>>;

    async fn add_group_devices(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        added_epoch: i64,
        devices: &[(DeviceKeyId, UserId)],
    ) -> RepoResult<()>;

    async fn remove_group_devices(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        device_key_ids: &[DeviceKeyId],
    ) -> RepoResult<()>;

    /// Set `conversation.encrypted_at` to now unless it is set. Returns whether it changed.
    async fn mark_conversation_encrypted(
        &self,
        conversation_id: ConversationId,
    ) -> RepoResult<bool>;

    /// Conversations whose current group has the device as a leaf.
    async fn conversations_with_group_device(
        &self,
        device_key_id: DeviceKeyId,
    ) -> RepoResult<Vec<ConversationId>>;

    /// Conversations with a current group in which the user is a member.
    async fn grouped_conversations_of_user(
        &self,
        user_id: UserId,
    ) -> RepoResult<Vec<ConversationId>>;

    // ── Commits ──────────────────────────────────────────────────────────

    /// # Errors
    /// `Conflict` when the group already has a Commit for this epoch.
    async fn insert_commit(&self, commit: NewMlsCommit) -> RepoResult<MlsCommit>;

    /// Commits of an incarnation after `after_epoch` (all when `None`), oldest first, at most
    /// `limit`.
    async fn list_commits(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        after_epoch: Option<i64>,
        limit: u32,
    ) -> RepoResult<Vec<MlsCommit>>;

    /// Whether a Commit of the incarnation was accepted after `since`.
    async fn commit_accepted_since(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        since: DateTime<Utc>,
    ) -> RepoResult<bool>;

    // ── Welcomes ─────────────────────────────────────────────────────────

    async fn insert_welcome(&self, welcome: NewMlsWelcome) -> RepoResult<MlsWelcomeId>;

    /// The device's unacknowledged, unexpired Welcomes to conversations its user is still a
    /// member of, oldest first.
    async fn pending_welcomes(&self, device_key_id: DeviceKeyId) -> RepoResult<Vec<MlsWelcome>>;

    /// Acknowledge the device's Welcomes among `ids`. Returns how many were newly acknowledged.
    async fn ack_welcomes(
        &self,
        device_key_id: DeviceKeyId,
        ids: &[MlsWelcomeId],
    ) -> RepoResult<u64>;

    // ── Re-add requests ──────────────────────────────────────────────────

    /// Record the request and its nonce together; `Conflict` when the device already used the
    /// nonce in this incarnation.
    async fn insert_readd_request(
        &self,
        request: NewMlsReaddRequest,
    ) -> RepoResult<MlsReaddRequest>;

    /// Unfulfilled requests of an incarnation made at or after `since`, oldest first.
    async fn open_readd_requests(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        since: DateTime<Utc>,
    ) -> RepoResult<Vec<MlsReaddRequest>>;

    /// Mark the devices' open requests fulfilled by the Commit that moved the group to `epoch`.
    async fn fulfill_readd_requests(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        device_key_ids: &[DeviceKeyId],
        epoch: i64,
    ) -> RepoResult<u64>;

    // ── Encrypted messages ───────────────────────────────────────────────

    /// # Errors
    /// `Conflict` when a message with this id exists.
    async fn insert_encrypted_message(&self, message: NewEncryptedMessage) -> RepoResult<Message>;

    /// Replace an encrypted message's envelope (an edit). The change feed trigger bumps its
    /// version and sets `edited_at`.
    async fn replace_envelope(
        &self,
        message_id: MessageId,
        envelope: MessageEncryption,
    ) -> RepoResult<Message>;

    // ── Franking ─────────────────────────────────────────────────────────

    /// The newest franking key, creating key 1 from `secret` when there is none.
    async fn current_franking_key(&self, secret: Vec<u8>) -> RepoResult<FrankingKey>;

    async fn get_franking_key(&self, id: i32) -> RepoResult<Option<FrankingKey>>;

    // ── Retention ────────────────────────────────────────────────────────

    /// Delete Commits, expired Welcomes and key packages, claimed one-time key packages,
    /// fulfilled or old re-add requests, and clear the franked content of long-resolved reports.
    async fn purge(&self, retention: MlsRetention) -> RepoResult<MlsPurged>;
}
