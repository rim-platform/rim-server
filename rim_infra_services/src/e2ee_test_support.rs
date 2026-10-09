//! Test doubles for end-to-end encryption: an in-memory MLS store and a wire whose "MLS
//! messages" are JSON descriptions of their framing.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rim_core::conversation::model::ConversationId;
use rim_core::e2ee::model::{
    ClaimedKeyPackage, E2eeDevice, FrankingKey, KEY_PACKAGE_MIN_REMAINING_DAYS, MlsCommit,
    MlsCommitId, MlsDesiredDevice, MlsDevice, MlsGroup, MlsGroupDevice, MlsKeyPackage,
    MlsKeyPackageId, MlsKeyPackageStatus, MlsPurged, MlsReaddRequest, MlsReaddRequestId,
    MlsRetention, MlsWelcome, MlsWelcomeId, NewEncryptedMessage, NewMlsCommit, NewMlsGroup,
    NewMlsKeyPackage, NewMlsReaddRequest, NewMlsWelcome,
};
use rim_core::e2ee::repository::{E2eeRepository, GroupLock};
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::message::model::{Message, MessageEncryption, MessageId};
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_services::e2ee::wire::{
    KeyPackageInfo, MlsContentType, MlsWire, MlsWireError, PrivateMessageInfo, WelcomeInfo,
};

use crate::e2ee::openmls_wire::OpenMlsWire;

pub(crate) const SUITE: i32 = 0x004f;

pub(crate) fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

// ── In-memory MLS storage ─────────────────────────────────────────────────────────────

#[derive(Clone)]
pub(crate) struct StoredPackage {
    pub(crate) meta: MlsKeyPackage,
    pub(crate) bytes: Vec<u8>,
    pub(crate) claims: i32,
}

pub(crate) type Welcomes = Vec<(MlsWelcomeId, NewMlsWelcome, HashSet<DeviceKeyId>)>;

#[derive(Default)]
pub(crate) struct State {
    pub(crate) devices: Vec<E2eeDevice>,
    pub(crate) members: HashMap<ConversationId, Vec<UserId>>,
    pub(crate) principals: HashMap<UserId, String>,
    pub(crate) visible: HashMap<UserId, HashSet<UserId>>,
    pub(crate) packages: Vec<StoredPackage>,
    pub(crate) groups: Vec<MlsGroup>,
    pub(crate) group_devices: Vec<(ConversationId, i32, MlsGroupDevice)>,
    pub(crate) commits: Vec<MlsCommit>,
    pub(crate) welcomes: Welcomes,
    pub(crate) readds: Vec<MlsReaddRequest>,
    /// Re-add nonces used, per conversation, incarnation and device.
    pub(crate) readd_nonces: HashSet<(ConversationId, i32, DeviceKeyId, Vec<u8>)>,
    pub(crate) encrypted: HashSet<ConversationId>,
    pub(crate) franking: Vec<FrankingKey>,
    pub(crate) fail_commit_insert: bool,
    /// Encrypted messages, by id.
    pub(crate) messages: HashMap<MessageId, Message>,
}

#[derive(Default)]
pub(crate) struct FakeE2ee(Mutex<State>);

impl FakeE2ee {
    pub(crate) fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().expect("state")
    }

    fn mls_device(state: &State, id: DeviceKeyId) -> Option<MlsDevice> {
        let device = state.devices.iter().find(|d| d.device_key_id == id)?;
        Some(MlsDevice {
            device_key_id: id,
            user_id: device.user_id,
            principal_id: device.principal_id.clone().unwrap_or_default(),
            device_id: device.device_id,
        })
    }

    fn active(device: &E2eeDevice, since: DateTime<Utc>) -> bool {
        device.revoked_at.is_none()
            && device.principal_id.is_some()
            && device
                .last_key_package_upload_at
                .is_some_and(|at| at >= since)
    }

    fn available(p: &StoredPackage, device: DeviceKeyId, suite: i32, now: DateTime<Utc>) -> bool {
        p.meta.device_key_id == device
            && p.meta.cipher_suite == suite
            && !p.meta.last_resort
            && p.meta.claimed_at.is_none()
            && Self::usable(p, now)
    }

    fn usable(p: &StoredPackage, now: DateTime<Utc>) -> bool {
        p.meta.not_after > now + chrono::Duration::days(KEY_PACKAGE_MIN_REMAINING_DAYS)
    }
}

#[async_trait]
impl E2eeRepository for FakeE2ee {
    async fn get_device(&self, id: DeviceKeyId) -> RepoResult<Option<E2eeDevice>> {
        Ok(self
            .state()
            .devices
            .iter()
            .find(|d| d.device_key_id == id)
            .cloned())
    }

    async fn find_device(
        &self,
        principal_id: &str,
        device_id: Uuid,
    ) -> RepoResult<Option<E2eeDevice>> {
        Ok(self
            .state()
            .devices
            .iter()
            .find(|d| d.principal_id.as_deref() == Some(principal_id) && d.device_id == device_id)
            .cloned())
    }

    async fn lock_device(&self, id: DeviceKeyId) -> RepoResult<bool> {
        Ok(self.state().devices.iter().any(|d| d.device_key_id == id))
    }

    async fn touch_key_package_upload(&self, id: DeviceKeyId) -> RepoResult<()> {
        let mut state = self.state();
        if let Some(d) = state.devices.iter_mut().find(|d| d.device_key_id == id) {
            d.last_key_package_upload_at = Some(Utc::now());
        }
        Ok(())
    }

    async fn ready_users(
        &self,
        user_ids: &[UserId],
        active_since: DateTime<Utc>,
    ) -> RepoResult<Vec<UserId>> {
        let state = self.state();
        Ok(user_ids
            .iter()
            .copied()
            .filter(|u| {
                state
                    .devices
                    .iter()
                    .any(|d| d.user_id == *u && Self::active(d, active_since))
            })
            .collect())
    }

    async fn mls_active_device_ids(
        &self,
        user_id: UserId,
        active_since: DateTime<Utc>,
    ) -> RepoResult<Vec<Uuid>> {
        Ok(self
            .state()
            .devices
            .iter()
            .filter(|d| d.user_id == user_id && Self::active(d, active_since))
            .map(|d| d.device_id)
            .collect())
    }

    async fn desired_devices(
        &self,
        conversation_id: ConversationId,
        active_since: DateTime<Utc>,
        cipher_suite: i32,
    ) -> RepoResult<Vec<MlsDesiredDevice>> {
        let state = self.state();
        let members = state
            .members
            .get(&conversation_id)
            .cloned()
            .unwrap_or_default();
        let now = Utc::now();
        Ok(state
            .devices
            .iter()
            .filter(|d| members.contains(&d.user_id) && Self::active(d, active_since))
            .map(|d| MlsDesiredDevice {
                device: Self::mls_device(&state, d.device_key_id).expect("device"),
                has_key_package: state.packages.iter().any(|p| {
                    p.meta.device_key_id == d.device_key_id
                        && p.meta.cipher_suite == cipher_suite
                        && Self::usable(p, now)
                        && (p.meta.last_resort || p.meta.claimed_at.is_none())
                }),
            })
            .collect())
    }

    async fn members_by_principal(
        &self,
        conversation_id: ConversationId,
        principal_ids: &[String],
    ) -> RepoResult<Vec<(String, UserId)>> {
        let state = self.state();
        let members = state
            .members
            .get(&conversation_id)
            .cloned()
            .unwrap_or_default();
        Ok(members
            .into_iter()
            .filter_map(|u| {
                let p = state.principals.get(&u)?;
                principal_ids.contains(p).then(|| (p.clone(), u))
            })
            .collect())
    }

    async fn principals_visible_to(
        &self,
        viewer: UserId,
        principal_ids: &[String],
    ) -> RepoResult<Vec<(String, UserId)>> {
        let state = self.state();
        let visible = state.visible.get(&viewer).cloned().unwrap_or_default();
        Ok(state
            .principals
            .iter()
            .filter(|(u, p)| (**u == viewer || visible.contains(u)) && principal_ids.contains(p))
            .map(|(u, p)| (p.clone(), *u))
            .collect())
    }

    async fn count_available_key_packages(
        &self,
        device_key_id: DeviceKeyId,
        cipher_suite: i32,
    ) -> RepoResult<i64> {
        let now = Utc::now();
        Ok(self
            .state()
            .packages
            .iter()
            .filter(|p| Self::available(p, device_key_id, cipher_suite, now))
            .count() as i64)
    }

    async fn delete_last_resort_key_package(
        &self,
        device_key_id: DeviceKeyId,
        cipher_suite: i32,
    ) -> RepoResult<u64> {
        let mut state = self.state();
        let before = state.packages.len();
        state.packages.retain(|p| {
            !(p.meta.device_key_id == device_key_id
                && p.meta.cipher_suite == cipher_suite
                && p.meta.last_resort)
        });
        Ok((before - state.packages.len()) as u64)
    }

    async fn insert_key_package(&self, package: NewMlsKeyPackage) -> RepoResult<MlsKeyPackageId> {
        let mut state = self.state();
        let duplicate = state.packages.iter().any(|p| {
            p.meta.key_package_ref == package.key_package_ref
                || (package.last_resort
                    && p.meta.last_resort
                    && p.meta.device_key_id == package.device_key_id
                    && p.meta.cipher_suite == package.cipher_suite)
        });
        if duplicate {
            return Err(RepoError::new(RepoErrorKind::Conflict));
        }
        let id = MlsKeyPackageId::new();
        state.packages.push(StoredPackage {
            meta: MlsKeyPackage {
                id,
                user_id: package.user_id,
                device_key_id: package.device_key_id,
                cipher_suite: package.cipher_suite,
                key_package_ref: package.key_package_ref,
                last_resort: package.last_resort,
                not_after: package.not_after,
                created_at: Utc::now(),
                claimed_at: None,
                claimed_by_device_key_id: None,
                claimed_for_conversation_id: None,
                claimed_for_incarnation: None,
            },
            bytes: package.key_package,
            claims: 0,
        });
        Ok(id)
    }

    async fn key_package_status(
        &self,
        device_key_id: DeviceKeyId,
    ) -> RepoResult<Vec<MlsKeyPackageStatus>> {
        let available = self
            .count_available_key_packages(device_key_id, SUITE)
            .await?;
        let state = self.state();
        let mine: Vec<_> = state
            .packages
            .iter()
            .filter(|p| p.meta.device_key_id == device_key_id)
            .collect();
        if mine.is_empty() {
            return Ok(vec![]);
        }
        Ok(vec![MlsKeyPackageStatus {
            cipher_suite: SUITE,
            available,
            last_resort_not_after: mine
                .iter()
                .filter(|p| p.meta.last_resort)
                .map(|p| p.meta.not_after)
                .max(),
            last_resort_claims: mine
                .iter()
                .filter(|p| p.meta.last_resort)
                .map(|p| p.claims)
                .max()
                .unwrap_or(0),
            oldest_not_after: mine
                .iter()
                .filter(|p| Self::available(p, device_key_id, SUITE, Utc::now()))
                .map(|p| p.meta.not_after)
                .min(),
        }])
    }

    async fn claim_one_time_key_package(
        &self,
        device_key_id: DeviceKeyId,
        cipher_suite: i32,
        claimed_by: DeviceKeyId,
        conversation_id: ConversationId,
        incarnation: i32,
        uploaded_after: Option<DateTime<Utc>>,
    ) -> RepoResult<Option<ClaimedKeyPackage>> {
        let mut state = self.state();
        let now = Utc::now();
        let mut candidates: Vec<&mut StoredPackage> = state
            .packages
            .iter_mut()
            .filter(|p| Self::available(p, device_key_id, cipher_suite, now))
            .filter(|p| uploaded_after.is_none_or(|at| p.meta.created_at > at))
            .collect();
        let found = if uploaded_after.is_some() {
            candidates.pop()
        } else {
            candidates.into_iter().next()
        };
        Ok(found.map(|p| {
            p.meta.claimed_at = Some(now);
            p.meta.claimed_by_device_key_id = Some(claimed_by);
            p.meta.claimed_for_conversation_id = Some(conversation_id);
            p.meta.claimed_for_incarnation = Some(incarnation);
            ClaimedKeyPackage {
                id: p.meta.id,
                device_key_id,
                key_package_ref: p.meta.key_package_ref.clone(),
                key_package: p.bytes.clone(),
                last_resort: false,
            }
        }))
    }

    async fn claim_last_resort_key_package(
        &self,
        device_key_id: DeviceKeyId,
        cipher_suite: i32,
        claimed_by: DeviceKeyId,
        conversation_id: ConversationId,
        incarnation: i32,
    ) -> RepoResult<Option<ClaimedKeyPackage>> {
        let mut state = self.state();
        let now = Utc::now();
        let found = state.packages.iter_mut().find(|p| {
            p.meta.device_key_id == device_key_id
                && p.meta.cipher_suite == cipher_suite
                && p.meta.last_resort
                && Self::usable(p, now)
        });
        Ok(found.map(|p| {
            p.meta.claimed_at = Some(now);
            p.meta.claimed_by_device_key_id = Some(claimed_by);
            p.meta.claimed_for_conversation_id = Some(conversation_id);
            p.meta.claimed_for_incarnation = Some(incarnation);
            p.claims += 1;
            ClaimedKeyPackage {
                id: p.meta.id,
                device_key_id,
                key_package_ref: p.meta.key_package_ref.clone(),
                key_package: p.bytes.clone(),
                last_resort: true,
            }
        }))
    }

    async fn find_key_packages_by_refs(&self, refs: &[Vec<u8>]) -> RepoResult<Vec<MlsKeyPackage>> {
        Ok(self
            .state()
            .packages
            .iter()
            .filter(|p| refs.contains(&p.meta.key_package_ref))
            .map(|p| p.meta.clone())
            .collect())
    }

    async fn consume_key_packages(&self, ids: &[MlsKeyPackageId]) -> RepoResult<u64> {
        let mut state = self.state();
        let before = state.packages.len();
        state
            .packages
            .retain(|p| p.meta.last_resort || !ids.contains(&p.meta.id));
        Ok((before - state.packages.len()) as u64)
    }

    async fn current_group(&self, conversation_id: ConversationId) -> RepoResult<Option<MlsGroup>> {
        Ok(self
            .state()
            .groups
            .iter()
            .find(|g| g.conversation_id == conversation_id && g.retired_at.is_none())
            .cloned())
    }

    async fn lock_current_group(
        &self,
        conversation_id: ConversationId,
        _lock: GroupLock,
    ) -> RepoResult<Option<MlsGroup>> {
        self.current_group(conversation_id).await
    }

    async fn insert_group(&self, group: NewMlsGroup) -> RepoResult<MlsGroup> {
        let mut state = self.state();
        if state.groups.iter().any(|g| {
            g.conversation_id == group.conversation_id
                && (g.incarnation == group.incarnation || g.retired_at.is_none())
        }) {
            return Err(RepoError::new(RepoErrorKind::Conflict));
        }
        let stored = MlsGroup {
            conversation_id: group.conversation_id,
            incarnation: group.incarnation,
            group_id: group.group_id,
            cipher_suite: group.cipher_suite,
            epoch: group.epoch,
            created_by_device_key_id: group.created_by_device_key_id,
            created_at: Utc::now(),
            retired_at: None,
        };
        state.groups.push(stored.clone());
        Ok(stored)
    }

    async fn retire_group(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
    ) -> RepoResult<()> {
        let mut state = self.state();
        for g in &mut state.groups {
            if g.conversation_id == conversation_id && g.incarnation == incarnation {
                g.retired_at = Some(Utc::now());
            }
        }
        Ok(())
    }

    async fn advance_epoch(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
    ) -> RepoResult<i64> {
        let mut state = self.state();
        let group = state
            .groups
            .iter_mut()
            .find(|g| g.conversation_id == conversation_id && g.incarnation == incarnation)
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
        group.epoch += 1;
        Ok(group.epoch)
    }

    async fn group_devices(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
    ) -> RepoResult<Vec<MlsGroupDevice>> {
        Ok(self
            .state()
            .group_devices
            .iter()
            .filter(|(c, i, _)| *c == conversation_id && *i == incarnation)
            .map(|(_, _, d)| d.clone())
            .collect())
    }

    async fn add_group_devices(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        added_epoch: i64,
        devices: &[(DeviceKeyId, UserId)],
    ) -> RepoResult<()> {
        let mut state = self.state();
        for (id, _) in devices {
            let device = Self::mls_device(&state, *id).expect("device");
            state.group_devices.push((
                conversation_id,
                incarnation,
                MlsGroupDevice {
                    device,
                    added_epoch,
                },
            ));
        }
        Ok(())
    }

    async fn remove_group_devices(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        ids: &[DeviceKeyId],
    ) -> RepoResult<()> {
        self.state().group_devices.retain(|(c, i, d)| {
            !(*c == conversation_id && *i == incarnation && ids.contains(&d.device.device_key_id))
        });
        Ok(())
    }

    async fn mark_conversation_encrypted(
        &self,
        conversation_id: ConversationId,
    ) -> RepoResult<bool> {
        Ok(self.state().encrypted.insert(conversation_id))
    }

    async fn conversations_with_group_device(
        &self,
        device_key_id: DeviceKeyId,
    ) -> RepoResult<Vec<ConversationId>> {
        let state = self.state();
        Ok(state
            .group_devices
            .iter()
            .filter(|(c, i, d)| {
                d.device.device_key_id == device_key_id
                    && state.groups.iter().any(|g| {
                        g.conversation_id == *c && g.incarnation == *i && g.retired_at.is_none()
                    })
            })
            .map(|(c, _, _)| *c)
            .collect())
    }

    async fn grouped_conversations_of_user(
        &self,
        user_id: UserId,
    ) -> RepoResult<Vec<ConversationId>> {
        let state = self.state();
        Ok(state
            .groups
            .iter()
            .filter(|g| {
                g.retired_at.is_none()
                    && state
                        .members
                        .get(&g.conversation_id)
                        .is_some_and(|m| m.contains(&user_id))
            })
            .map(|g| g.conversation_id)
            .collect())
    }

    async fn insert_commit(&self, commit: NewMlsCommit) -> RepoResult<MlsCommit> {
        let mut state = self.state();
        if state.fail_commit_insert
            || state.commits.iter().any(|c| {
                c.conversation_id == commit.conversation_id
                    && c.incarnation == commit.incarnation
                    && c.epoch == commit.epoch
            })
        {
            return Err(RepoError::new(RepoErrorKind::Conflict));
        }
        let device = Self::mls_device(&state, commit.sender_device_key_id);
        let stored = MlsCommit {
            id: MlsCommitId::new(),
            conversation_id: commit.conversation_id,
            incarnation: commit.incarnation,
            epoch: commit.epoch,
            sender_user_id: Some(commit.sender_user_id),
            sender_device_key_id: Some(commit.sender_device_key_id),
            sender_principal_id: device.as_ref().map(|d| d.principal_id.clone()),
            sender_device_id: device.map(|d| d.device_id),
            message: commit.message,
            created_at: Utc::now(),
        };
        state.commits.push(stored.clone());
        Ok(stored)
    }

    async fn list_commits(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        after_epoch: Option<i64>,
        limit: u32,
    ) -> RepoResult<Vec<MlsCommit>> {
        let mut commits: Vec<_> = self
            .state()
            .commits
            .iter()
            .filter(|c| {
                c.conversation_id == conversation_id
                    && c.incarnation == incarnation
                    && after_epoch.is_none_or(|e| c.epoch > e)
            })
            .cloned()
            .collect();
        commits.sort_by_key(|c| c.epoch);
        commits.truncate(limit as usize);
        Ok(commits)
    }

    async fn commit_accepted_since(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        since: DateTime<Utc>,
    ) -> RepoResult<bool> {
        Ok(self.state().commits.iter().any(|c| {
            c.conversation_id == conversation_id
                && c.incarnation == incarnation
                && c.created_at > since
        }))
    }

    async fn insert_welcome(&self, welcome: NewMlsWelcome) -> RepoResult<MlsWelcomeId> {
        let id = MlsWelcomeId::new();
        self.state().welcomes.push((id, welcome, HashSet::new()));
        Ok(id)
    }

    async fn pending_welcomes(&self, device_key_id: DeviceKeyId) -> RepoResult<Vec<MlsWelcome>> {
        Ok(self
            .state()
            .welcomes
            .iter()
            .filter(|(_, w, acked)| {
                w.recipients.iter().any(|(d, _)| *d == device_key_id)
                    && !acked.contains(&device_key_id)
            })
            .map(|(id, w, _)| MlsWelcome {
                id: *id,
                server_id: ServerId::new(),
                conversation_id: w.conversation_id,
                incarnation: w.incarnation,
                epoch: w.epoch,
                message: w.message.clone(),
                created_at: Utc::now(),
                expires_at: w.expires_at,
            })
            .collect())
    }

    async fn ack_welcomes(
        &self,
        device_key_id: DeviceKeyId,
        ids: &[MlsWelcomeId],
    ) -> RepoResult<u64> {
        let mut acked = 0;
        for (id, w, done) in &mut self.state().welcomes {
            if ids.contains(id)
                && w.recipients.iter().any(|(d, _)| *d == device_key_id)
                && done.insert(device_key_id)
            {
                acked += 1;
            }
        }
        Ok(acked)
    }

    async fn insert_readd_request(
        &self,
        request: NewMlsReaddRequest,
    ) -> RepoResult<MlsReaddRequest> {
        let mut state = self.state();
        if !state.readd_nonces.insert((
            request.conversation_id,
            request.incarnation,
            request.device_key_id,
            request.nonce.clone(),
        )) {
            return Err(RepoError::new(RepoErrorKind::Conflict));
        }
        let stored = MlsReaddRequest {
            id: MlsReaddRequestId::new(),
            conversation_id: request.conversation_id,
            incarnation: request.incarnation,
            device: Self::mls_device(&state, request.device_key_id).expect("device"),
            payload: request.payload,
            signature: request.signature,
            created_at: Utc::now(),
            fulfilled_epoch: None,
        };
        state.readds.push(stored.clone());
        Ok(stored)
    }

    async fn open_readd_requests(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        since: DateTime<Utc>,
    ) -> RepoResult<Vec<MlsReaddRequest>> {
        Ok(self
            .state()
            .readds
            .iter()
            .filter(|r| {
                r.conversation_id == conversation_id
                    && r.incarnation == incarnation
                    && r.fulfilled_epoch.is_none()
                    && r.created_at >= since
            })
            .cloned()
            .collect())
    }

    async fn fulfill_readd_requests(
        &self,
        conversation_id: ConversationId,
        incarnation: i32,
        ids: &[DeviceKeyId],
        epoch: i64,
    ) -> RepoResult<u64> {
        let mut n = 0;
        for r in &mut self.state().readds {
            if r.conversation_id == conversation_id
                && r.incarnation == incarnation
                && ids.contains(&r.device.device_key_id)
                && r.fulfilled_epoch.is_none()
            {
                r.fulfilled_epoch = Some(epoch);
                n += 1;
            }
        }
        Ok(n)
    }

    async fn insert_encrypted_message(&self, message: NewEncryptedMessage) -> RepoResult<Message> {
        let mut state = self.state();
        if state.messages.contains_key(&message.id) {
            return Err(RepoError::new(RepoErrorKind::Conflict));
        }
        let stored = Message {
            id: message.id,
            conversation_id: message.conversation_id,
            thread_id: message.thread_id,
            parent_message_id: None,
            forwarded_from_message_id: message.forwarded_from_message_id,
            sent_at: message.sent_at,
            sent_by: message.sent_by,
            text: None,
            is_deleted: false,
            has_attachments: false,
            encryption: Some(message.envelope),
        };
        state.messages.insert(stored.id, stored.clone());
        Ok(stored)
    }

    async fn replace_envelope(
        &self,
        id: MessageId,
        envelope: MessageEncryption,
    ) -> RepoResult<Message> {
        let mut state = self.state();
        let message = state
            .messages
            .get_mut(&id)
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
        message.encryption = Some(envelope);
        Ok(message.clone())
    }

    async fn legacy_franking_keys(&self) -> RepoResult<Vec<FrankingKey>> {
        Ok(self.state().franking.clone())
    }

    async fn max_message_franking_key_id(&self) -> RepoResult<Option<i32>> {
        Ok(self
            .state()
            .messages
            .values()
            .filter_map(|m| m.encryption.as_ref().map(|e| e.franking_key_id))
            .max())
    }

    async fn purge(&self, _retention: MlsRetention) -> RepoResult<MlsPurged> {
        Ok(MlsPurged::default())
    }
}

// ── A wire whose "MLS messages" are JSON descriptions of their framing ────────────────

#[derive(Serialize, Deserialize)]
pub(crate) struct FakeFrame {
    pub(crate) group_id: Vec<u8>,
    pub(crate) epoch: u64,
    pub(crate) content: String,
    pub(crate) aad: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct FakeWelcome {
    pub(crate) suite: i32,
    pub(crate) refs: Vec<Vec<u8>>,
}

/// Fake framing for Commits and Welcomes; real OpenMLS for key packages.
pub(crate) struct FakeWire(pub(crate) OpenMlsWire);

impl MlsWire for FakeWire {
    fn read_key_package(&self, bytes: &[u8]) -> Result<KeyPackageInfo, MlsWireError> {
        self.0.read_key_package(bytes)
    }

    fn read_private_message(&self, bytes: &[u8]) -> Result<PrivateMessageInfo, MlsWireError> {
        let frame: FakeFrame = serde_json::from_slice(bytes)
            .map_err(|_| MlsWireError("not a valid MLSMessage".to_string()))?;
        Ok(PrivateMessageInfo {
            group_id: frame.group_id,
            epoch: frame.epoch,
            content_type: match frame.content.as_str() {
                "commit" => MlsContentType::Commit,
                "proposal" => MlsContentType::Proposal,
                _ => MlsContentType::Application,
            },
            authenticated_data: frame.aad.into_bytes(),
        })
    }

    fn read_welcome(&self, bytes: &[u8]) -> Result<WelcomeInfo, MlsWireError> {
        let welcome: FakeWelcome = serde_json::from_slice(bytes)
            .map_err(|_| MlsWireError("the MLSMessage is not a Welcome".to_string()))?;
        Ok(WelcomeInfo {
            cipher_suite: welcome.suite,
            new_member_refs: welcome.refs,
        })
    }
}

pub(crate) fn frame(group_id: Vec<u8>, epoch: u64, content: &str, aad: &str) -> String {
    b64(&serde_json::to_vec(&FakeFrame {
        group_id,
        epoch,
        content: content.to_string(),
        aad: aad.to_string(),
    })
    .expect("frame"))
}

pub(crate) fn welcome_bytes(refs: Vec<Vec<u8>>) -> String {
    b64(&serde_json::to_vec(&FakeWelcome { suite: SUITE, refs }).expect("welcome"))
}

pub(crate) const EMPTY_COMMIT: &str = r#"{"v":1,"type":"commit","adds":[],"removes":[]}"#;
