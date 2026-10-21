use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rim_infra_db::TransactionScope;
use sea_orm::DatabaseConnection;
use tracing::{info, instrument};

/// Maximum number of items any paginated query can return in a single page.
const MAX_PAGE_LIMIT: u32 = 200;

/// Maximum number of conversations one message change feed request may name.
const MAX_CHANGE_FEED_CONVERSATIONS: usize = 100;

/// Maximum number of changes one message change feed request returns across all its
/// conversations: the per-conversation limit is lowered to fit, and `has_more` carries the rest.
const MAX_CHANGE_FEED_TOTAL: u32 = 1000;

use rim_core::conversation::model::Conversation;
use rim_core::e2ee::model::{NewEncryptedMessage, NewFrankingRevision};
use rim_core::e2ee::repository::{E2eeRepository, GroupLock};
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::message::model::{Message, MessageEncryption, MessageId, NewMessage};
use rim_core::message::repository::MessageSearchParams;
use rim_core::message_thread::model::MessageThread;
use rim_core::moderation::repository::ModerationRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::repository::pagination::{Page, PageDir, SortOrder};
use rim_core::user::model::UserId;
use rim_infra_db::begin_transaction;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::e2ee::wire::{MlsContentType, MlsWire};
use rim_services::e2ee::{MAX_ENVELOPE_BYTES, MlsCommitRequiredReason};
use rim_services::events::{EventObserver, MessageEventData, Observers, WorkspaceEvent};
use rim_services::limits::{InstanceLimits, limit_reached};
use rim_services::message::command::{
    DeleteMessageCmd, EditEncryptedMessageCmd, EditMessageCmd, ForwardMessageCmd, GetMessageQuery,
    ListMessageChangesQuery, ListMessagesQuery, ListThreadMessagesQuery, ReplyToMessageCmd,
    SearchMessagesQuery, SendEncryptedMessageCmd, SendMessageCmd,
};
use rim_services::message::dto::{
    ConversationChangesStatus, ConversationMessageChangesDto, MessageChangeDto, MessageChangeKind,
    MessageDto, MessageEncryptionDto, MessagePageCursor, MessageSearchHitDto,
};
use rim_services::message::search_query::{ChannelToken, FromToken, HasFilter, parse_search_query};
use rim_services::message::service::MessageService;
use rim_services::message_attachment::dto::AttachmentDto;
use rim_services::notification::command::EncryptedMentions;
use rim_services::{ServiceError, ServiceResult};

/// Maximum allowed length (in characters) for message text.
const MAX_MESSAGE_TEXT_LENGTH: usize = 10_000;

/// Bytes an encrypted message's ciphertext may carry beyond the instance's message-bytes cap:
/// the MLS framing, signature and tag, and the client's content structure around the text.
const ENCRYPTED_FRAMING_ALLOWANCE: usize = 4096;

/// Seconds a member must still wait under slow mode, or `None` when they may send now: the
/// interval minus the time since `last_sent_at`, rounded up (never less than 1 while waiting).
pub(crate) fn slow_mode_retry_after(
    interval_seconds: i32,
    last_sent_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<u64> {
    if interval_seconds <= 0 {
        return None;
    }
    let interval_ms = i64::from(interval_seconds) * 1000;
    let elapsed_ms = (now - last_sent_at).num_milliseconds().max(0);
    if elapsed_ms >= interval_ms {
        return None;
    }
    let remaining_ms = interval_ms - elapsed_ms;
    u64::try_from((remaining_ms + 999) / 1000).ok()
}

use crate::MessageRepos;
use crate::clock::Clock;
use crate::e2ee::aad::{ApplicationAad, parse_application};
use crate::e2ee::franking::{ReceiptFields, receipt, sent_at_us};
use crate::e2ee::{
    GroupState, check_group_id, decode_b64, read_private_message, require_device, session_device,
    stale,
};
use crate::franking_keys::SharedFrankingKeys;
use crate::mention::parse_mention_ids;
use crate::moderation::{ModerationAuditEntry, audit_ids, record_moderation_action};
use crate::repos::AppRepos;
use crate::token_bucket::{BucketRate, TokenBucketLimiter};
use crate::util::{
    ConversationAccessRepos, ensure_conversation_member_access, ensure_conversation_writable,
    ensure_conversation_writable_with_standing, ensure_not_archived, member_standing,
};

/// The refusal of a plaintext write into an encrypted conversation.
fn encrypted_conversation() -> ServiceError {
    ServiceError::Validation("this conversation is encrypted".to_string())
}

/// What encrypted messages need besides the message repositories: the MLS storage, bound to
/// the message's transaction, and the framing decoder.
#[derive(Clone)]
pub(crate) struct MessageE2ee {
    pub e2ee_repo: Arc<dyn E2eeRepository>,
    pub wire: Arc<dyn MlsWire>,
    pub franking_keys: SharedFrankingKeys,
}

/// An encrypted message's envelope and authenticated data, checked against the request.
struct DecodedEnvelope {
    bytes: Vec<u8>,
    group_id: Vec<u8>,
    epoch: i64,
    aad: ApplicationAad,
}

#[derive(Clone)]
pub struct DefaultMessageService {
    repos: MessageRepos,
    db: Option<DatabaseConnection>,
    observers: Observers,
    /// Writes the `admin.delete_message` audit row for moderator deletes. `None` in unit tests;
    /// the composition root always wires it.
    audit_log_service: Option<std::sync::Arc<dyn rim_services::audit::service::AuditLogService>>,
    /// Decides and records notifications after a message commits, best-effort.
    /// `None` in unit tests that are not about notifications; the composition root wires it.
    notification_service:
        Option<std::sync::Arc<dyn rim_services::notification::service::NotificationService>>,
    /// Encrypted messages. `None` in unit tests that are not about them; the composition root
    /// wires it.
    e2ee: Option<MessageE2ee>,
    /// Lets a moderator delete a message with a verified report outside its conversation.
    /// `None` in unit tests that are not about it; the composition root wires it.
    moderation_repo: Option<Arc<dyn ModerationRepository>>,
    /// Messages each user may send per minute, across every way of creating one. `None` sets
    /// no limit.
    message_budget: Option<Arc<TokenBucketLimiter<UserId>>>,
    /// Messages each account in probation may send per minute, instead of `message_budget`.
    /// `None` keeps them on `message_budget`.
    probation_budget: Option<Arc<TokenBucketLimiter<UserId>>>,
    /// The cap on a message's text in bytes, and the probation of new accounts.
    limits: InstanceLimits,
    clock: Clock,
    /// This server's handle domain: `from:@name:<it>` names a local account.
    handle_domain: Option<String>,
}

impl DefaultMessageService {
    pub fn new(repos: MessageRepos, db: Option<DatabaseConnection>, observers: Observers) -> Self {
        Self {
            repos,
            db,
            observers,
            audit_log_service: None,
            notification_service: None,
            e2ee: None,
            moderation_repo: None,
            message_budget: None,
            probation_budget: None,
            limits: InstanceLimits::default(),
            clock: Clock::default(),
            handle_domain: None,
        }
    }

    /// Resolve `from:@name:<handle_domain>` searches to local accounts.
    pub fn with_handle_domain(mut self, handle_domain: Option<String>) -> Self {
        self.handle_domain = handle_domain;
        self
    }

    /// The message rate of accounts in probation.
    pub fn with_probation_budget(mut self, rate: BucketRate) -> Self {
        self.probation_budget = Some(Arc::new(TokenBucketLimiter::new(rate)));
        self
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// The budget `author`'s messages draw on: the probation one while their account is in
    /// probation, else the normal one.
    async fn budget_for(
        &self,
        author: UserId,
    ) -> ServiceResult<Option<&Arc<TokenBucketLimiter<UserId>>>> {
        if let Some(probation) = &self.probation_budget
            && self.limits.probation_hours.is_some()
            && let Some(user) = self.repos.user_repo.get_by_id(author).await?
            && crate::probation::probation_ends_at(
                &self.limits,
                self.repos.user_repo.as_ref(),
                &user,
                self.clock.now(),
            )
            .await?
            .is_some()
        {
            return Ok(Some(probation));
        }
        Ok(self.message_budget.as_ref())
    }

    pub fn with_limits(mut self, limits: InstanceLimits) -> Self {
        self.limits = limits;
        self
    }

    /// `Validation` when `text` is longer than the instance allows, in bytes.
    fn ensure_text_bytes(&self, text: &str) -> ServiceResult<()> {
        match self.limits.message_bytes {
            Some(cap) if text.len() > cap as usize => {
                Err(limit_reached(cap, "bytes of text per message"))
            }
            _ => Ok(()),
        }
    }

    /// `Validation` when an encrypted message's ciphertext, its envelope less the
    /// authenticated data, is longer than the instance's text cap plus
    /// [`ENCRYPTED_FRAMING_ALLOWANCE`].
    pub(crate) fn ensure_ciphertext_bytes(&self, envelope: usize, aad: usize) -> ServiceResult<()> {
        match self.limits.message_bytes {
            Some(cap)
                if envelope.saturating_sub(aad) > cap as usize + ENCRYPTED_FRAMING_ALLOWANCE =>
            {
                Err(limit_reached(cap, "bytes of text per message"))
            }
            _ => Ok(()),
        }
    }

    pub fn with_message_budget(mut self, rate: BucketRate) -> Self {
        self.message_budget = Some(Arc::new(TokenBucketLimiter::new(rate)));
        self
    }

    /// `RateLimited` once `author` has used their message budget.
    async fn ensure_message_budget(&self, author: UserId) -> ServiceResult<()> {
        let Some(budget) = self.budget_for(author).await? else {
            return Ok(());
        };
        match budget.check(&author, std::time::Instant::now()) {
            RateLimitDecision::Allowed => Ok(()),
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => {
                crate::metrics::rate_limited("messages");
                Err(ServiceError::RateLimited {
                    retry_after_seconds,
                })
            }
        }
    }

    pub fn with_moderation_repo(mut self, moderation_repo: Arc<dyn ModerationRepository>) -> Self {
        self.moderation_repo = Some(moderation_repo);
        self
    }

    /// The conversation and the actor's standing when an Owner/Moderator who cannot read the
    /// conversation deletes a message with a verified franked report; `None` otherwise.
    async fn reported_message_reach(
        &self,
        cmd: &DeleteMessageCmd,
    ) -> ServiceResult<Option<(Conversation, rim_core::server_member::model::MemberStanding)>> {
        let Some(moderation_repo) = &self.moderation_repo else {
            return Ok(None);
        };
        let Some(standing) = self
            .repos
            .server_member_repo
            .get_member_standing(cmd.server_id, cmd.actor.user_id)
            .await?
        else {
            return Ok(None);
        };
        if !standing.role.is_moderator()
            || !moderation_repo
                .has_verified_report(cmd.server_id, cmd.message_id)
                .await?
        {
            return Ok(None);
        }
        let conv = crate::util::ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;
        Ok(Some((conv, standing)))
    }

    pub fn with_e2ee(
        mut self,
        e2ee_repo: Arc<dyn E2eeRepository>,
        wire: Arc<dyn MlsWire>,
        franking_keys: SharedFrankingKeys,
    ) -> Self {
        self.e2ee = Some(MessageE2ee {
            e2ee_repo,
            wire,
            franking_keys,
        });
        self
    }

    fn e2ee(&self) -> ServiceResult<&MessageE2ee> {
        self.e2ee.as_ref().ok_or_else(|| {
            ServiceError::internal(anyhow::anyhow!("encrypted messages are not configured"))
        })
    }

    pub fn with_notification_service(
        mut self,
        notification_service: std::sync::Arc<
            dyn rim_services::notification::service::NotificationService,
        >,
    ) -> Self {
        self.notification_service = Some(notification_service);
        self
    }

    /// Best-effort notifications for a committed message: a failure is logged and
    /// never fails the send.
    async fn notify_recipients(
        &self,
        server_id: rim_core::server::model::ServerId,
        conversation: &rim_core::conversation::model::Conversation,
        message: Message,
        member_user_ids: Vec<rim_core::user::model::UserId>,
        encrypted_mentions: Option<EncryptedMentions>,
    ) {
        let Some(notification_service) = &self.notification_service else {
            return;
        };
        let message_id = message.id;
        if let Err(e) = notification_service
            .notify_for_message(rim_services::notification::command::NotifyForMessageCmd {
                server_id,
                conversation: conversation.clone(),
                message,
                member_user_ids,
                encrypted_mentions,
            })
            .await
        {
            tracing::warn!(error = %e, message_id = %message_id, "failed to create notifications");
        }
    }

    /// Best-effort: a deleted message's unread notifications go away.
    async fn forget_notifications(&self, message_id: rim_core::message::model::MessageId) {
        let Some(notification_service) = &self.notification_service else {
            return;
        };
        if let Err(e) = notification_service.forget_message(message_id).await {
            tracing::warn!(error = %e, message_id = %message_id, "failed to delete notifications of a deleted message");
        }
    }

    pub fn with_audit_log_service(
        mut self,
        audit_log_service: std::sync::Arc<dyn rim_services::audit::service::AuditLogService>,
    ) -> Self {
        self.audit_log_service = Some(audit_log_service);
        self
    }

    /// Slow mode: a non-moderator whose previous message in the conversation is
    /// younger than `conversation.slow_mode_seconds` gets `RateLimited` with the seconds left.
    /// Owner/Moderator are exempt. Reads the author's last `sent_at`; no in-memory state.
    async fn ensure_slow_mode_allows(
        &self,
        conversation: &rim_core::conversation::model::Conversation,
        standing: &rim_core::server_member::model::MemberStanding,
        author: rim_core::user::model::UserId,
    ) -> ServiceResult<()> {
        if conversation.slow_mode_seconds <= 0 || standing.role.is_moderator() {
            return Ok(());
        }
        let Some(last) = self
            .repos
            .message_repo
            .last_sent_at_by_author(conversation.id, author)
            .await?
        else {
            return Ok(());
        };
        match slow_mode_retry_after(conversation.slow_mode_seconds, last, chrono::Utc::now()) {
            Some(retry_after_seconds) => Err(ServiceError::RateLimited {
                retry_after_seconds,
            }),
            None => Ok(()),
        }
    }

    /// Begin a transaction if a database connection is available.
    ///
    /// Returns `(repos, optional_scope)`. The caller must commit the scope on
    /// success — if dropped without committing it rolls back automatically.
    async fn begin_write(&self) -> ServiceResult<(MessageRepos, Option<TransactionScope>)> {
        if let Some(db) = &self.db {
            let scope = begin_transaction(db)
                .await
                .map_err(ServiceError::internal)?;
            let repos = AppRepos::with_handle(&scope.handle()).for_message();
            Ok((repos, Some(scope)))
        } else {
            Ok((self.repos.clone(), None))
        }
    }

    /// Commit the transaction scope if present.
    async fn commit_if_present(scope: Option<TransactionScope>) -> ServiceResult<()> {
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        Ok(())
    }

    /// Parse mentions from message text, validate them against the users who can read the
    /// conversation, store in DB, and emit UserMentioned event.
    async fn process_mentions(
        &self,
        text: Option<&str>,
        message_id: rim_core::message::model::MessageId,
        conversation: &rim_core::conversation::model::Conversation,
        author_id: rim_core::user::model::UserId,
    ) {
        let Some(text) = text else { return };
        let mention_ids = parse_mention_ids(text);
        if mention_ids.is_empty() {
            return;
        }
        let server_id = conversation.server_id;
        let conversation_id = conversation.id;

        // Validate: each mentioned user must be a workspace member who can read the
        // conversation — anyone for a public channel, otherwise a conversation member. A
        // mention of anyone else would tell them (via `UserMentioned`) that a private
        // conversation exists.
        let mut valid_ids = Vec::new();
        for uid in &mention_ids {
            if *uid == author_id {
                continue; // Don't mention yourself
            }
            let Ok(Some(member)) = self
                .repos
                .server_member_repo
                .get_member(server_id, *uid)
                .await
            else {
                continue;
            };
            // A guest can read a public channel only as a conversation member.
            if (conversation.is_public_channel() && !member.member_role.is_guest())
                || matches!(
                    self.repos
                        .conversation_member_repo
                        .is_member(conversation_id, *uid)
                        .await,
                    Ok(true)
                )
            {
                valid_ids.push(*uid);
            }
        }

        if valid_ids.is_empty() {
            return;
        }

        if let Err(e) = self
            .repos
            .message_mention_repo
            .create_mentions(message_id, &valid_ids)
            .await
        {
            tracing::warn!(error = %e, "failed to store mentions");
            return;
        }

        self.observers.emit(WorkspaceEvent::UserMentioned {
            workspace_id: server_id,
            conversation_id,
            message_id,
            mentioned_user_ids: valid_ids,
            author_id,
        });
    }

    /// Collect conversation member IDs for event broadcasting.
    async fn list_member_user_ids(
        &self,
        conversation_id: rim_core::conversation::model::ConversationId,
    ) -> ServiceResult<Vec<rim_core::user::model::UserId>> {
        self.repos
            .conversation_member_repo
            .list_member_ids(conversation_id)
            .await
            .map_err(ServiceError::from)
    }

    async fn to_dto(
        &self,
        message: rim_core::message::model::Message,
        server_id: rim_core::server::model::ServerId,
    ) -> ServiceResult<MessageDto> {
        let root_thread_id = if message.thread_id.is_none() {
            self.repos
                .message_thread_repo
                .get_by_root_message(message.id)
                .await?
                .map(|thread| thread.id)
        } else {
            None
        };

        let attachments = self
            .repos
            .message_attachment_repo
            .get_attachments_for_message(message.id)
            .await?
            .into_iter()
            .map(crate::message_attachment::DefaultMessageAttachmentService::attachment_dto)
            .collect();
        let device_ids = self
            .sender_device_ids(std::slice::from_ref(&message))
            .await?;
        let encryption = Self::encryption_dto(&message, &device_ids);

        Ok(MessageDto {
            id: message.id,
            server_id,
            conversation_id: message.conversation_id,
            thread_id: message.thread_id,
            root_thread_id,
            author_id: message.sent_by,
            text: message.text,
            has_attachments: message.has_attachments,
            attachments,
            parent_message_id: message.parent_message_id,
            forwarded_from_message_id: message.forwarded_from_message_id,
            thread_root_message_id: None,
            is_deleted: message.is_deleted,
            sent_at: message.sent_at,
            encryption,
        })
    }

    /// The client device ids of the devices that sent the encrypted ones among `messages`.
    async fn sender_device_ids(
        &self,
        messages: &[Message],
    ) -> ServiceResult<HashMap<DeviceKeyId, uuid::Uuid>> {
        let mut ids = HashMap::new();
        let Some(e2ee) = &self.e2ee else {
            return Ok(ids);
        };
        let senders: std::collections::HashSet<DeviceKeyId> = messages
            .iter()
            .filter_map(|m| m.encryption.as_ref()?.sender_device_key_id)
            .collect();
        for device_key_id in senders {
            if let Some(device) = e2ee.e2ee_repo.get_device(device_key_id).await? {
                ids.insert(device_key_id, device.device_id);
            }
        }
        Ok(ids)
    }

    fn encryption_dto(
        message: &Message,
        device_ids: &HashMap<DeviceKeyId, uuid::Uuid>,
    ) -> Option<MessageEncryptionDto> {
        let encryption = message.encryption.as_ref()?;
        Some(MessageEncryptionDto {
            incarnation: encryption.incarnation,
            epoch: encryption.epoch,
            sender_device_id: encryption
                .sender_device_key_id
                .and_then(|id| device_ids.get(&id).copied())
                .unwrap_or_default(),
            ciphertext: encryption.ciphertext.clone(),
            franking_receipt: encryption.franking_receipt.clone(),
            franking_key_id: encryption.franking_key_id,
            sent_at_us: sent_at_us(message.sent_at),
        })
    }

    fn to_dto_with_root_thread_id(
        &self,
        message: rim_core::message::model::Message,
        server_id: rim_core::server::model::ServerId,
        root_thread_id: Option<rim_core::message_thread::model::MessageThreadId>,
        attachments: Vec<AttachmentDto>,
        thread_root_message_id: Option<rim_core::message::model::MessageId>,
        encryption: Option<MessageEncryptionDto>,
    ) -> MessageDto {
        MessageDto {
            id: message.id,
            server_id,
            conversation_id: message.conversation_id,
            thread_id: message.thread_id,
            root_thread_id,
            author_id: message.sent_by,
            text: message.text,
            has_attachments: message.has_attachments,
            attachments,
            parent_message_id: message.parent_message_id,
            forwarded_from_message_id: message.forwarded_from_message_id,
            thread_root_message_id,
            is_deleted: message.is_deleted,
            sent_at: message.sent_at,
            encryption,
        }
    }

    /// The thread rooted at `root_message_id`, created when it has none; `true` when created.
    async fn find_or_create_thread(
        repos: &MessageRepos,
        conversation_id: rim_core::conversation::model::ConversationId,
        root_message_id: MessageId,
    ) -> ServiceResult<(MessageThread, bool)> {
        if let Some(existing) = repos
            .message_thread_repo
            .get_by_root_message(root_message_id)
            .await?
        {
            return Ok((existing, false));
        }
        match repos
            .message_thread_repo
            .create_thread_for_root(conversation_id, root_message_id)
            .await
        {
            Ok(thread) => Ok((thread, true)),
            Err(err) if matches!(err.kind(), RepoErrorKind::Conflict) => {
                // Race condition: another request created the thread concurrently.
                let thread = repos
                    .message_thread_repo
                    .get_by_root_message(root_message_id)
                    .await?
                    .ok_or(ServiceError::Conflict(
                        "thread already exists for message".to_string(),
                    ))?;
                Ok((thread, false))
            }
            Err(err) => Err(ServiceError::from(err)),
        }
    }

    /// The origin of a forward: a message the caller can read that is not deleted.
    async fn ensure_forward_source_readable(
        &self,
        actor: UserId,
        server_id: rim_core::server::model::ServerId,
        source: MessageId,
    ) -> ServiceResult<()> {
        let message = self
            .repos
            .message_repo
            .get_by_id(source)
            .await?
            .ok_or(ServiceError::NotFound)?;
        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            actor,
            server_id,
            message.conversation_id,
        )
        .await?;
        if message.is_deleted {
            return Err(ServiceError::Validation(
                "cannot forward a deleted message".to_string(),
            ));
        }
        Ok(())
    }

    /// Begin a write that also touches MLS storage, in one transaction when a database is
    /// configured.
    async fn begin_encrypted_write(
        &self,
    ) -> ServiceResult<(
        MessageRepos,
        Arc<dyn E2eeRepository>,
        Option<TransactionScope>,
    )> {
        let e2ee = self.e2ee()?;
        match &self.db {
            Some(db) => {
                let scope = begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?;
                let repos = AppRepos::with_handle(&scope.handle());
                Ok((repos.for_message(), repos.e2ee_repo, Some(scope)))
            }
            None => Ok((self.repos.clone(), Arc::clone(&e2ee.e2ee_repo), None)),
        }
    }

    /// Decode an envelope and check that its authenticated data names `message_id`.
    fn decode_envelope(
        &self,
        envelope: &str,
        message_id: MessageId,
    ) -> ServiceResult<DecodedEnvelope> {
        let e2ee = self.e2ee()?;
        let bytes = decode_b64("envelope", envelope, MAX_ENVELOPE_BYTES)?;
        let header = read_private_message(e2ee.wire.as_ref(), &bytes, MlsContentType::Application)?;
        self.ensure_ciphertext_bytes(bytes.len(), header.authenticated_data.len())?;
        let aad = parse_application(&header.authenticated_data)?;
        if aad.message_id != uuid::Uuid::from(message_id) {
            return Err(ServiceError::Validation(
                "the authenticated data names another message".to_string(),
            ));
        }
        let epoch = i64::try_from(header.epoch)
            .map_err(|_| ServiceError::Validation("the epoch is out of range".to_string()))?;
        Ok(DecodedEnvelope {
            bytes,
            group_id: header.group_id,
            epoch,
            aad,
        })
    }

    /// Inside the message's transaction: the group must be at the envelope's incarnation and
    /// epoch, the sending device one of its leaves, and no removal pending. Returns the
    /// incarnation.
    async fn check_envelope_against_group(
        e2ee_repo: &dyn E2eeRepository,
        conversation: &Conversation,
        envelope: &DecodedEnvelope,
        device_key_id: DeviceKeyId,
        now: DateTime<Utc>,
    ) -> ServiceResult<i32> {
        let group = e2ee_repo
            .lock_current_group(conversation.id, GroupLock::Share)
            .await?
            .ok_or(ServiceError::MlsCommitRequired {
                reason: MlsCommitRequiredReason::NoGroup,
            })?;
        check_group_id(conversation.id, &group, &envelope.group_id)?;
        if envelope.epoch != group.epoch {
            return Err(stale(&group));
        }
        let state = GroupState::load(e2ee_repo, group, now).await?;
        let sender_pending_removal = state
            .pending_removes()
            .any(|d| d.device.device_key_id == device_key_id);
        if !state.is_device(device_key_id) || sender_pending_removal {
            return Err(ServiceError::Forbidden(
                "this device is not in the conversation's group".to_string(),
            ));
        }
        if state.pending_removes().next().is_some() {
            return Err(ServiceError::MlsCommitRequired {
                reason: MlsCommitRequiredReason::PendingRemovals,
            });
        }
        Ok(state.group.incarnation)
    }

    /// The members an encrypted message mentions, by principal id: current members only, the
    /// author skipped.
    async fn encrypted_mentions(
        e2ee_repo: &dyn E2eeRepository,
        conversation: &Conversation,
        aad: &ApplicationAad,
        author: UserId,
    ) -> ServiceResult<EncryptedMentions> {
        let mut user_ids: Vec<UserId> = e2ee_repo
            .members_by_principal(conversation.id, &aad.mentions)
            .await?
            .into_iter()
            .map(|(_, user_id)| user_id)
            .filter(|user_id| *user_id != author)
            .collect();
        user_ids.dedup();
        Ok(EncryptedMentions {
            user_ids,
            all: aad.mention_all,
        })
    }

    /// The franking receipt of an envelope and the id of the key that made it: the server's
    /// current franking key.
    fn frank(
        franking_keys: &SharedFrankingKeys,
        fields: ReceiptFields,
        commitment: &[u8],
    ) -> (i32, Vec<u8>) {
        let keys = franking_keys.keys();
        let key = keys.current();
        (key.id, receipt(key.secret(), &fields, commitment).to_vec())
    }

    async fn ensure_thread_in_conversation(
        &self,
        thread_id: rim_core::message_thread::model::MessageThreadId,
        conversation_id: rim_core::conversation::model::ConversationId,
    ) -> ServiceResult<rim_core::message_thread::model::MessageThread> {
        let thread = self
            .repos
            .message_thread_repo
            .get_by_id(thread_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if thread.conversation_id != conversation_id {
            return Err(ServiceError::NotFound);
        }

        Ok(thread)
    }

    async fn get_message_model(
        &self,
        actor: rim_services::Actor,
        server_id: rim_core::server::model::ServerId,
        conversation_id: rim_core::conversation::model::ConversationId,
        message_id: rim_core::message::model::MessageId,
    ) -> ServiceResult<rim_core::message::model::Message> {
        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            actor.user_id,
            server_id,
            conversation_id,
        )
        .await?;

        let msg = self
            .repos
            .message_repo
            .get_by_id(message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if msg.conversation_id != conversation_id {
            return Err(ServiceError::NotFound);
        }

        Ok(msg)
    }

    async fn map_messages_to_dtos(
        &self,
        server_id: rim_core::server::model::ServerId,
        messages: Vec<rim_core::message::model::Message>,
        include_root_thread_ids: bool,
    ) -> ServiceResult<Vec<MessageDto>> {
        let message_ids = messages
            .iter()
            .map(|message| message.id)
            .collect::<Vec<_>>();
        let attachments_by_message = self
            .repos
            .message_attachment_repo
            .get_attachments_for_messages(&message_ids)
            .await?;
        let root_thread_ids = if include_root_thread_ids {
            self.repos
                .message_thread_repo
                .get_thread_ids_by_root_messages(&message_ids)
                .await?
        } else {
            std::collections::HashMap::new()
        };

        // Batch-load threads for messages that are thread replies, to
        // resolve thread_root_message_id (= thread.first_message_id).
        let thread_ids_from_messages: Vec<rim_core::message_thread::model::MessageThreadId> =
            messages
                .iter()
                .filter_map(|m| m.thread_id)
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
        let thread_root_message_map: std::collections::HashMap<
            rim_core::message_thread::model::MessageThreadId,
            rim_core::message::model::MessageId,
        > = if !thread_ids_from_messages.is_empty() {
            let threads = self
                .repos
                .message_thread_repo
                .get_by_ids(&thread_ids_from_messages)
                .await?;
            threads
                .into_iter()
                .filter_map(|t| t.first_message_id.map(|mid| (t.id, mid)))
                .collect()
        } else {
            std::collections::HashMap::new()
        };

        let device_ids = self.sender_device_ids(&messages).await?;
        let mut items = Vec::with_capacity(messages.len());
        for message in messages {
            let encryption = Self::encryption_dto(&message, &device_ids);
            let root_thread_id = if include_root_thread_ids && message.thread_id.is_none() {
                root_thread_ids.get(&message.id).copied()
            } else {
                None
            };
            let thread_root_message_id = message
                .thread_id
                .and_then(|tid| thread_root_message_map.get(&tid).copied());
            let attachments = attachments_by_message
                .get(&message.id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(crate::message_attachment::DefaultMessageAttachmentService::attachment_dto)
                .collect();
            items.push(self.to_dto_with_root_thread_id(
                message,
                server_id,
                root_thread_id,
                attachments,
                thread_root_message_id,
                encryption,
            ));
        }

        Ok(items)
    }
}

#[async_trait]
impl MessageService for DefaultMessageService {
    #[instrument(
        name = "message.send_message",
        skip_all,
        fields(
            actor_id = %cmd.actor.user_id,
            server_id = %cmd.server_id,
            conversation_id = %cmd.conversation_id,
            parent_message_id = ?cmd.parent_message_id,
            text_len = cmd.text.as_ref().map(|t| t.len()).unwrap_or(0)
        )
    )]
    async fn send_message(&self, cmd: SendMessageCmd) -> ServiceResult<MessageDto> {
        self.ensure_message_budget(cmd.actor.user_id).await?;
        let (conv, standing) = ensure_conversation_writable_with_standing(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            cmd.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;
        if conv.is_encrypted() {
            return Err(encrypted_conversation());
        }
        self.ensure_slow_mode_allows(&conv, &standing, cmd.actor.user_id)
            .await?;

        if let Some(source) = cmd.forwarded_from_message_id {
            self.ensure_forward_source_readable(cmd.actor.user_id, cmd.server_id, source)
                .await?;
        }

        let text = cmd.text.as_deref().map(str::trim).map(ToString::to_string);
        let text = match text {
            Some(t) if !t.is_empty() => Some(t),
            _ => None, // Allow empty text (e.g. attachment-only messages)
        };

        if let Some(ref t) = text
            && t.chars().count() > MAX_MESSAGE_TEXT_LENGTH
        {
            return Err(ServiceError::Validation(format!(
                "message text too long (max {} characters)",
                MAX_MESSAGE_TEXT_LENGTH
            )));
        }
        if let Some(ref t) = text {
            self.ensure_text_bytes(t)?;
        }

        let parent_message_id = match cmd.parent_message_id {
            None => None,
            Some(parent_id) => {
                let parent = self
                    .repos
                    .message_repo
                    .get_by_id(parent_id)
                    .await?
                    .ok_or(ServiceError::NotFound)?;

                if parent.conversation_id != cmd.conversation_id || parent.thread_id.is_some() {
                    return Err(ServiceError::NotFound);
                }
                Some(parent_id)
            }
        };

        let new_message = NewMessage {
            conversation_id: cmd.conversation_id,
            text,
            sent_by: cmd.actor.user_id,
            thread_id: None,
            parent_message_id,
            forwarded_from_message_id: cmd.forwarded_from_message_id,
        };

        // Run create + activity touch inside a transaction when DB is available.
        let (repos, scope) = self.begin_write().await?;

        let message = repos.message_repo.create_message(&new_message).await?;

        repos
            .conversation_repo
            .touch_last_activity_at(cmd.conversation_id, message.sent_at)
            .await?;

        Self::commit_if_present(scope).await?;
        crate::metrics::message_created(false);

        info!(message_id = %message.id, is_reply = parent_message_id.is_some(), "created timeline message");
        info!(
            conversation_id = %cmd.conversation_id,
            "updated conversation activity"
        );

        let dto = self.to_dto(message.clone(), cmd.server_id).await?;

        let member_user_ids = self.list_member_user_ids(cmd.conversation_id).await?;

        self.observers.emit(WorkspaceEvent::NewMessage {
            workspace_id: cmd.server_id,
            conversation_id: dto.conversation_id,
            conversation_type: conv.conversation_type.as_str().to_string(),
            is_private: conv.is_private,
            member_user_ids: member_user_ids.clone(),
            message: MessageEventData::from(&dto),
        });

        self.process_mentions(dto.text.as_deref(), dto.id, &conv, cmd.actor.user_id)
            .await;
        self.notify_recipients(cmd.server_id, &conv, message, member_user_ids, None)
            .await;

        Ok(dto)
    }

    #[instrument(
        name = "message.reply_to_message",
        skip_all,
        fields(
            actor_id = %cmd.actor.user_id,
            server_id = %cmd.server_id,
            conversation_id = %cmd.conversation_id,
            root_message_id = %cmd.root_message_id,
            parent_message_id = ?cmd.parent_message_id,
            text_len = cmd.text.len()
        )
    )]
    async fn reply_to_message(&self, cmd: ReplyToMessageCmd) -> ServiceResult<MessageDto> {
        self.ensure_message_budget(cmd.actor.user_id).await?;
        // 1. Validate access (membership, archive, timeout) and slow mode
        let (conv, standing) = ensure_conversation_writable_with_standing(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            cmd.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;
        if conv.is_encrypted() {
            return Err(encrypted_conversation());
        }
        self.ensure_slow_mode_allows(&conv, &standing, cmd.actor.user_id)
            .await?;

        // 2. Validate root message exists and belongs to conversation
        let root_message = self
            .repos
            .message_repo
            .get_by_id(cmd.root_message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if root_message.conversation_id != cmd.conversation_id || root_message.thread_id.is_some() {
            return Err(ServiceError::NotFound);
        }

        // 3. Validate text

        let text = cmd.text.trim().to_string();
        if text.is_empty() {
            return Err(ServiceError::Validation(
                "message text must be non-empty".to_string(),
            ));
        }

        if text.chars().count() > MAX_MESSAGE_TEXT_LENGTH {
            return Err(ServiceError::Validation(format!(
                "message text too long (max {} characters)",
                MAX_MESSAGE_TEXT_LENGTH
            )));
        }
        self.ensure_text_bytes(&text)?;

        // 4. Begin transaction
        let (repos, scope) = self.begin_write().await?;

        // 5. Find or create thread
        let (thread, thread_was_created) =
            Self::find_or_create_thread(&repos, cmd.conversation_id, cmd.root_message_id).await?;

        // Validate parent_message_id if provided
        let parent_message_id = match cmd.parent_message_id {
            None => None,
            Some(parent_id) => {
                let parent = repos
                    .message_repo
                    .get_by_id(parent_id)
                    .await?
                    .ok_or(ServiceError::NotFound)?;

                let is_root_parent =
                    thread.first_message_id == Some(parent.id) && parent.thread_id.is_none();
                let is_thread_parent = parent.thread_id == Some(thread.id);
                let valid_parent = is_root_parent || is_thread_parent;

                if parent.conversation_id != cmd.conversation_id || !valid_parent {
                    return Err(ServiceError::NotFound);
                }
                Some(parent_id)
            }
        };

        // 6. Create message with thread_id
        let new_msg = NewMessage {
            conversation_id: cmd.conversation_id,
            text: Some(text),
            sent_by: cmd.actor.user_id,
            thread_id: Some(thread.id),
            parent_message_id,
            forwarded_from_message_id: None,
        };

        let message = repos.message_repo.create_message(&new_msg).await?;

        // 7. Touch thread last_message_at
        repos
            .message_thread_repo
            .touch_last_message_at(thread.id, message.sent_at)
            .await?;

        // 8. Touch conversation last_activity_at
        repos
            .conversation_repo
            .touch_last_activity_at(cmd.conversation_id, message.sent_at)
            .await?;

        // 9. Commit transaction
        Self::commit_if_present(scope).await?;
        crate::metrics::message_created(false);

        info!(
            message_id = %message.id,
            thread_id = %thread.id,
            thread_was_created,
            "created reply message"
        );

        let dto = self.to_dto(message.clone(), cmd.server_id).await?;

        let member_user_ids = self.list_member_user_ids(cmd.conversation_id).await?;

        // 10. Emit events
        if thread_was_created {
            self.observers.emit(WorkspaceEvent::ThreadCreated {
                workspace_id: cmd.server_id,
                conversation_id: cmd.conversation_id,
                thread_id: thread.id,
                root_message_id: cmd.root_message_id,
                conversation_type: conv.conversation_type.as_str().to_string(),
                is_private: conv.is_private,
                member_user_ids: member_user_ids.clone(),
            });
        }

        self.observers.emit(WorkspaceEvent::NewMessage {
            workspace_id: cmd.server_id,
            conversation_id: dto.conversation_id,
            conversation_type: conv.conversation_type.as_str().to_string(),
            is_private: conv.is_private,
            member_user_ids: member_user_ids.clone(),
            message: MessageEventData::from(&dto),
        });

        self.process_mentions(dto.text.as_deref(), dto.id, &conv, cmd.actor.user_id)
            .await;
        // 11. Notifications (best-effort, after commit)
        self.notify_recipients(cmd.server_id, &conv, message, member_user_ids, None)
            .await;

        Ok(dto)
    }

    async fn get_message(&self, query: GetMessageQuery) -> ServiceResult<MessageDto> {
        let msg = self
            .get_message_model(
                query.actor,
                query.server_id,
                query.conversation_id,
                query.message_id,
            )
            .await?;

        if msg.thread_id.is_some() {
            return Err(ServiceError::NotFound);
        }

        self.to_dto(msg, query.server_id).await
    }

    async fn list_messages(
        &self,
        query: ListMessagesQuery,
    ) -> ServiceResult<rim_core::repository::pagination::Page<MessageDto, MessagePageCursor>> {
        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            query.actor.user_id,
            query.server_id,
            query.conversation_id,
        )
        .await?;

        let mut page = self
            .repos
            .message_repo
            .list_messages(rim_core::message::repository::ListMessagesParams {
                conversation_id: query.conversation_id,
                thread_id: None,
                limit: query.limit.clamp(1, MAX_PAGE_LIMIT),
                dir: query.dir,
                cursor: query.cursor.map(Into::into),
                include_deleted: query.include_deleted,
                order: SortOrder::Desc,
            })
            .await?;

        // Reverse items to return in display order (oldest-first)
        page.items.reverse();

        let items = self
            .map_messages_to_dtos(query.server_id, page.items, true)
            .await?;

        Ok(rim_core::repository::pagination::Page::new(
            items,
            page.next.map(Into::into),
            page.prev.map(Into::into),
        ))
    }

    async fn list_thread_messages(
        &self,
        query: ListThreadMessagesQuery,
    ) -> ServiceResult<rim_core::repository::pagination::Page<MessageDto, MessagePageCursor>> {
        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            query.actor.user_id,
            query.server_id,
            query.conversation_id,
        )
        .await?;
        self.ensure_thread_in_conversation(query.thread_id, query.conversation_id)
            .await?;

        let mut page = self
            .repos
            .message_repo
            .list_messages(rim_core::message::repository::ListMessagesParams {
                conversation_id: query.conversation_id,
                thread_id: Some(query.thread_id),
                limit: query.limit.clamp(1, MAX_PAGE_LIMIT),
                dir: query.dir,
                cursor: query.cursor.map(Into::into),
                include_deleted: query.include_deleted,
                order: SortOrder::Desc,
            })
            .await?;

        // Reverse items to return in display order (oldest-first)
        page.items.reverse();

        let items = self
            .map_messages_to_dtos(query.server_id, page.items, false)
            .await?;

        Ok(rim_core::repository::pagination::Page::new(
            items,
            page.next.map(Into::into),
            page.prev.map(Into::into),
        ))
    }

    /// Full-text search. See [`rim_services::message::search_query`] for the operator syntax and
    /// `rim_core::message::repository::MessageSearchParams` for the visibility contract this
    /// method is responsible for upholding: the repository only ever searches the conversation
    /// set this method hands it, derived from `list_accessible_conversation_ids` (plus the member
    /// gate for an explicit `conversation_id` scope). `from:`/`in:` tokens that don't resolve, or
    /// an `in:` channel outside the accessible set, return an **empty page**, never an error —
    /// this avoids leaking whether a username/channel exists to an actor who can't see it.
    async fn search_messages(
        &self,
        query: SearchMessagesQuery,
    ) -> ServiceResult<Page<MessageSearchHitDto, MessagePageCursor>> {
        let parsed = parse_search_query(&query.query)?;

        // Resolve `from:` to a concrete UserId. Unknown username => empty page.
        let author_id = match &parsed.from {
            Some(FromToken::UserId(uuid)) => Some((*uuid).into()),
            Some(FromToken::Username(name)) => {
                match self.repos.user_repo.find_by_username(name).await? {
                    Some(user) => Some(user.id),
                    None => return Ok(Page::new(vec![], None, None)),
                }
            }
            Some(FromToken::Handle { username, domain }) => {
                let local = self
                    .handle_domain
                    .as_deref()
                    .is_some_and(|own| own.eq_ignore_ascii_case(domain));
                let user = if local {
                    self.repos.user_repo.find_by_username(username).await?
                } else {
                    self.repos
                        .user_repo
                        .find_by_home_handle(username, domain)
                        .await?
                };
                match user {
                    Some(user) => Some(user.id),
                    None => return Ok(Page::new(vec![], None, None)),
                }
            }
            None => None,
        };

        // Resolve `in:` to a concrete ConversationId. Unknown/non-channel => empty page.
        let in_channel_id = match &parsed.in_channel {
            Some(ChannelToken::ConversationId(uuid)) => Some((*uuid).into()),
            Some(ChannelToken::Name(name)) => {
                match self
                    .repos
                    .conversation_repo
                    .get_channel_by_title(query.server_id, name)
                    .await?
                {
                    Some(conv) => Some(conv.id),
                    None => return Ok(Page::new(vec![], None, None)),
                }
            }
            None => None,
        };

        // Compute the accessible conversation set — the security-critical step.
        let conversation_ids = if let Some(explicit_conversation_id) = query.conversation_id {
            // Explicit single-conversation scope: full three-step access gate.
            ensure_conversation_member_access(
                ConversationAccessRepos::from_arcs(
                    &self.repos.server_member_repo,
                    &self.repos.conversation_repo,
                    &self.repos.conversation_member_repo,
                ),
                query.actor.user_id,
                query.server_id,
                explicit_conversation_id,
            )
            .await?;
            vec![explicit_conversation_id]
        } else {
            // Workspace-wide search: server membership gates access;
            // list_accessible_conversation_ids already scopes to joined conversations. A guest
            // reads only conversations they are a member of, so public channels they have not
            // been added to are excluded.
            let standing = crate::util::member_standing(
                self.repos.server_member_repo.as_ref(),
                query.server_id,
                query.actor.user_id,
            )
            .await?;

            let accessible_ids = if standing.role.is_guest() {
                self.repos
                    .conversation_repo
                    .list_joined_conversation_ids(query.server_id, query.actor.user_id)
                    .await?
            } else {
                self.repos
                    .conversation_repo
                    .list_accessible_conversation_ids(query.server_id, query.actor.user_id)
                    .await?
            };

            match in_channel_id {
                // `in:` intersects the accessible set. A channel outside it never leaks —
                // return empty rather than a permission error.
                Some(channel_id) if accessible_ids.contains(&channel_id) => vec![channel_id],
                Some(_) => return Ok(Page::new(vec![], None, None)),
                None => accessible_ids,
            }
        };

        let clamped_limit = query.limit.clamp(1, MAX_PAGE_LIMIT);
        let mut hits = self
            .repos
            .message_repo
            .search_messages_fts(MessageSearchParams {
                conversation_ids,
                tsquery: parsed.text,
                author_id,
                has_link: parsed.has.contains(&HasFilter::Link),
                has_attachment: parsed.has.contains(&HasFilter::Attachment),
                after: parsed.after,
                before: parsed.before,
                limit: clamped_limit,
                cursor: query.cursor.map(Into::into),
                dir: query.dir,
            })
            .await?;

        // Boundary computation mirrors the (SortOrder::Desc, dir) branches in
        // SeaOrmMessageRepository::list_messages — search results are always ordered
        // (sent_at DESC, id DESC), so only those two branches apply. The repo already fetched
        // up to `clamped_limit + 1` rows so a next page can be detected here.
        let has_cursor = query.cursor.is_some();
        let has_more_in_dir = hits.len() > clamped_limit as usize;
        if has_more_in_dir {
            // Drop the extra row, which is the one farthest from the cursor (see
            // `MessageRepository::search_messages_fts`): the newest one for `Before`.
            if has_cursor && query.dir == PageDir::Before {
                hits.remove(0);
            } else {
                hits.truncate(clamped_limit as usize);
            }
        }

        let to_cursor = |hit: &rim_core::message::repository::MessageSearchHit| MessagePageCursor {
            sent_at: hit.message.sent_at,
            message_id: hit.message.id,
        };
        let min_cursor = hits.last().map(to_cursor);
        let max_cursor = hits.first().map(to_cursor);
        let has_opposite = has_cursor && !hits.is_empty();
        let (next, prev) = match query.dir {
            PageDir::After => (
                has_more_in_dir.then_some(min_cursor).flatten(),
                has_opposite.then_some(max_cursor).flatten(),
            ),
            PageDir::Before => (
                has_more_in_dir.then_some(max_cursor).flatten(),
                has_opposite.then_some(min_cursor).flatten(),
            ),
        };

        let ranks_and_headlines: Vec<(f32, Option<String>)> = hits
            .iter()
            .map(|hit| (hit.rank, hit.headline.clone()))
            .collect();
        let messages: Vec<Message> = hits.into_iter().map(|hit| hit.message).collect();
        let message_dtos = self
            .map_messages_to_dtos(query.server_id, messages, true)
            .await?;
        let items: Vec<MessageSearchHitDto> = message_dtos
            .into_iter()
            .zip(ranks_and_headlines)
            .map(|(message, (rank, headline))| MessageSearchHitDto {
                message,
                rank,
                headline,
            })
            .collect();

        Ok(Page::new(items, next, prev))
    }

    async fn list_message_changes(
        &self,
        query: ListMessageChangesQuery,
    ) -> ServiceResult<Vec<ConversationMessageChangesDto>> {
        if query.conversations.len() > MAX_CHANGE_FEED_CONVERSATIONS {
            return Err(ServiceError::Validation(format!(
                "too many conversations (max {MAX_CHANGE_FEED_CONVERSATIONS})"
            )));
        }
        if query.conversations.iter().any(|c| c.after_version < 0) {
            return Err(ServiceError::Validation(
                "change feed cursor must not be negative".to_string(),
            ));
        }
        // A caller outside the workspace fails as a whole. Below that, access is judged per
        // conversation and reported on its entry, so losing one channel does not stall the sync
        // of the others.
        member_standing(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let conversation_count = u32::try_from(query.conversations.len()).unwrap_or(u32::MAX);
        let limit = query
            .limit
            .clamp(1, MAX_PAGE_LIMIT)
            .min((MAX_CHANGE_FEED_TOTAL / conversation_count.max(1)).max(1));
        let mut result = Vec::with_capacity(query.conversations.len());
        for cursor in query.conversations {
            match ensure_conversation_member_access(
                ConversationAccessRepos::from_arcs(
                    &self.repos.server_member_repo,
                    &self.repos.conversation_repo,
                    &self.repos.conversation_member_repo,
                ),
                query.actor.user_id,
                query.server_id,
                cursor.conversation_id,
            )
            .await
            {
                Ok(_) => {}
                Err(ServiceError::NotFound | ServiceError::PermissionDenied) => {
                    result.push(ConversationMessageChangesDto {
                        conversation_id: cursor.conversation_id,
                        status: ConversationChangesStatus::NoAccess,
                        changes: Vec::new(),
                        cursor: cursor.after_version,
                        has_more: false,
                    });
                    continue;
                }
                Err(err) => return Err(err),
            }

            // A cursor past the newest version means the counters were reset since the client
            // stored it; reading on would silently skip changes, so start this conversation over.
            let reset = cursor.after_version > 0
                && cursor.after_version
                    > self
                        .repos
                        .message_repo
                        .latest_change_version(cursor.conversation_id)
                        .await?;
            let (status, after_version) = if reset {
                (ConversationChangesStatus::Reset, 0)
            } else {
                (ConversationChangesStatus::Ok, cursor.after_version)
            };
            let mut rows = self
                .repos
                .message_repo
                .list_message_changes(cursor.conversation_id, after_version, limit)
                .await?;
            let has_more = rows.len() > limit as usize;
            rows.truncate(limit as usize);

            let next_cursor = rows.last().map_or(after_version, |row| row.version);
            let (meta, messages): (Vec<_>, Vec<_>) = rows
                .into_iter()
                .map(|row| ((row.version, row.edited_at), row.message))
                .unzip();
            let dtos = self
                .map_messages_to_dtos(query.server_id, messages, true)
                .await?;
            let changes = meta
                .into_iter()
                .zip(dtos)
                .map(|((version, edited_at), message)| MessageChangeDto {
                    kind: if message.is_deleted {
                        MessageChangeKind::Deleted
                    } else if edited_at.is_some() {
                        MessageChangeKind::Edited
                    } else {
                        MessageChangeKind::Created
                    },
                    version,
                    edited_at,
                    message,
                })
                .collect();

            result.push(ConversationMessageChangesDto {
                conversation_id: cursor.conversation_id,
                status,
                changes,
                cursor: next_cursor,
                has_more,
            });
        }

        Ok(result)
    }

    async fn edit_message(&self, cmd: EditMessageCmd) -> ServiceResult<MessageDto> {
        let conv = ensure_conversation_writable(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            cmd.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;

        let new_text = cmd.text.trim();
        if new_text.is_empty() {
            return Err(ServiceError::Validation(
                "message text must be non-empty".to_string(),
            ));
        }

        if new_text.chars().count() > MAX_MESSAGE_TEXT_LENGTH {
            return Err(ServiceError::Validation(format!(
                "message text too long (max {} characters)",
                MAX_MESSAGE_TEXT_LENGTH
            )));
        }
        self.ensure_text_bytes(new_text)?;

        // Run validation + update inside a transaction.
        let (repos, scope) = self.begin_write().await?;

        let msg = repos
            .message_repo
            .get_by_id(cmd.message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if msg.conversation_id != cmd.conversation_id {
            return Err(ServiceError::NotFound);
        }

        if msg.sent_by != cmd.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        if msg.encryption.is_some() {
            return Err(encrypted_conversation());
        }

        if msg.is_deleted {
            return Err(ServiceError::Validation(
                "cannot edit a deleted message".to_string(),
            ));
        }

        let updated = repos
            .message_repo
            .update_message_text(cmd.message_id, new_text.to_string())
            .await?;

        Self::commit_if_present(scope).await?;

        let dto = self.to_dto(updated, cmd.server_id).await?;

        let member_user_ids = self.list_member_user_ids(cmd.conversation_id).await?;

        self.observers.emit(WorkspaceEvent::MessageUpdated {
            workspace_id: cmd.server_id,
            conversation_id: dto.conversation_id,
            conversation_type: conv.conversation_type.as_str().to_string(),
            is_private: conv.is_private,
            member_user_ids,
            message: MessageEventData::from(&dto),
        });

        // Re-process mentions: delete old, create new.
        let _ = self
            .repos
            .message_mention_repo
            .delete_mentions_for_message(cmd.message_id)
            .await;
        self.process_mentions(dto.text.as_deref(), dto.id, &conv, cmd.actor.user_id)
            .await;

        Ok(dto)
    }

    #[instrument(
        name = "message.forward_message",
        skip_all,
        fields(
            actor_id = %cmd.actor.user_id,
            server_id = %cmd.server_id,
            source_conversation_id = %cmd.source_conversation_id,
            target_conversation_id = %cmd.target_conversation_id,
            message_id = %cmd.message_id
        )
    )]
    async fn forward_message(&self, cmd: ForwardMessageCmd) -> ServiceResult<MessageDto> {
        self.ensure_message_budget(cmd.actor.user_id).await?;
        let access_repos = ConversationAccessRepos::from_arcs(
            &self.repos.server_member_repo,
            &self.repos.conversation_repo,
            &self.repos.conversation_member_repo,
        );
        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            cmd.actor.user_id,
            cmd.server_id,
            cmd.source_conversation_id,
        )
        .await?;
        let (target_conv, standing) = ensure_conversation_writable_with_standing(
            access_repos,
            cmd.actor.user_id,
            cmd.server_id,
            cmd.target_conversation_id,
        )
        .await?;
        // A forward is a send into the target: slow mode applies there too.
        self.ensure_slow_mode_allows(&target_conv, &standing, cmd.actor.user_id)
            .await?;

        // Get the source message
        let source_msg = self
            .repos
            .message_repo
            .get_by_id(cmd.message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if source_msg.conversation_id != cmd.source_conversation_id {
            return Err(ServiceError::NotFound);
        }

        if source_msg.is_deleted {
            return Err(ServiceError::Validation(
                "cannot forward a deleted message".to_string(),
            ));
        }

        // Encrypted messages are forwarded by sending their content again.
        if source_msg.encryption.is_some() {
            return Err(ServiceError::Validation(
                "an encrypted message is forwarded by sending its content".to_string(),
            ));
        }
        if target_conv.is_encrypted() {
            return Err(encrypted_conversation());
        }

        // Create the forwarded message in the target conversation
        let new_message = NewMessage {
            conversation_id: cmd.target_conversation_id,
            text: source_msg.text.clone(),
            sent_by: cmd.actor.user_id,
            thread_id: None,
            parent_message_id: None,
            forwarded_from_message_id: Some(cmd.message_id),
        };

        let (repos, scope) = self.begin_write().await?;

        let message = repos.message_repo.create_message(&new_message).await?;

        repos
            .conversation_repo
            .touch_last_activity_at(cmd.target_conversation_id, message.sent_at)
            .await?;

        Self::commit_if_present(scope).await?;
        crate::metrics::message_created(false);

        info!(
            message_id = %message.id,
            forwarded_from = %cmd.message_id,
            "forwarded message"
        );

        let dto = self.to_dto(message.clone(), cmd.server_id).await?;

        let member_user_ids = self
            .list_member_user_ids(cmd.target_conversation_id)
            .await?;

        self.observers.emit(WorkspaceEvent::NewMessage {
            workspace_id: cmd.server_id,
            conversation_id: dto.conversation_id,
            conversation_type: target_conv.conversation_type.as_str().to_string(),
            is_private: target_conv.is_private,
            member_user_ids: member_user_ids.clone(),
            message: MessageEventData::from(&dto),
        });

        // A forward is a send into the target: it notifies like one.
        self.notify_recipients(cmd.server_id, &target_conv, message, member_user_ids, None)
            .await;

        Ok(dto)
    }

    async fn delete_message(&self, cmd: DeleteMessageCmd) -> ServiceResult<()> {
        // Read gate plus archive, but not the timeout: a timed-out author may still delete
        // their own messages.
        let readable = crate::util::ensure_conversation_readable_with_standing(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            cmd.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await;
        let (conv, standing) = match readable {
            Ok(reach) => reach,
            Err(e @ (ServiceError::PermissionDenied | ServiceError::NotFound)) => {
                match self.reported_message_reach(&cmd).await? {
                    Some(reach) => reach,
                    None => return Err(e),
                }
            }
            Err(e) => return Err(e),
        };
        ensure_not_archived(&conv)?;

        if cmd.hard {
            return Err(ServiceError::Validation(
                "hard delete is not supported in v0".to_string(),
            ));
        }

        let msg = self
            .repos
            .message_repo
            .get_by_id(cmd.message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if msg.conversation_id != cmd.conversation_id {
            return Err(ServiceError::NotFound);
        }

        // The author, or an Owner/Moderator of the workspace.
        let moderator_delete = msg.sent_by != cmd.actor.user_id;
        if moderator_delete && !standing.role.is_moderator() {
            return Err(ServiceError::PermissionDenied);
        }

        let thread_id = msg.thread_id;

        self.repos
            .message_repo
            .soft_delete_message(cmd.message_id)
            .await?;
        self.forget_notifications(cmd.message_id).await;

        if moderator_delete {
            // Ids only: the message text is never copied into the audit log.
            record_moderation_action(
                self.audit_log_service.as_ref(),
                ModerationAuditEntry {
                    actor: cmd.actor.user_id,
                    server_id: cmd.server_id,
                    command_id: audit_ids::DELETE_MESSAGE,
                    args: serde_json::json!({
                        "conversation_id": cmd.conversation_id.to_string(),
                        "message_id": cmd.message_id.to_string(),
                        "author_id": msg.sent_by.to_string(),
                    }),
                    dangerous: true,
                    typed_verb: rim_services::command::verb::TypedVerb::Delete,
                },
            )
            .await;
        }

        let member_user_ids = self.list_member_user_ids(cmd.conversation_id).await?;

        self.observers.emit(WorkspaceEvent::MessageDeleted {
            workspace_id: cmd.server_id,
            conversation_id: cmd.conversation_id,
            conversation_type: conv.conversation_type.as_str().to_string(),
            is_private: conv.is_private,
            member_user_ids,
            message_id: cmd.message_id,
            thread_id,
        });

        Ok(())
    }

    #[instrument(
        name = "message.send_encrypted_message",
        skip_all,
        fields(
            actor_id = %cmd.caller.actor.user_id,
            conversation_id = %cmd.conversation_id,
            message_id = %cmd.message_id
        )
    )]
    async fn send_encrypted_message(
        &self,
        cmd: SendEncryptedMessageCmd,
    ) -> ServiceResult<MessageDto> {
        self.ensure_message_budget(cmd.caller.actor.user_id).await?;
        let e2ee = self.e2ee()?;
        require_device(&cmd.caller)?;
        let author = cmd.caller.actor.user_id;
        let conv = ensure_conversation_writable(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            author,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;
        if !conv.is_encrypted() {
            return Err(ServiceError::Validation(
                "this conversation is not encrypted".to_string(),
            ));
        }
        if uuid::Uuid::from(cmd.message_id).get_version_num() != 7 {
            return Err(ServiceError::Validation(
                "messageId must be a UUID v7".to_string(),
            ));
        }
        let envelope = self.decode_envelope(&cmd.envelope, cmd.message_id)?;

        // A retry of a send that was stored answers the stored message.
        if let Some(existing) = self.repos.message_repo.get_by_id(cmd.message_id).await? {
            let same = existing.conversation_id == conv.id
                && existing.sent_by == author
                && existing
                    .encryption
                    .as_ref()
                    .and_then(|e| e.ciphertext.as_deref())
                    == Some(envelope.bytes.as_slice());
            if same {
                return self.to_dto(existing, cmd.server_id).await;
            }
            return Err(ServiceError::Conflict(
                "the message id is taken".to_string(),
            ));
        }
        let device = session_device(e2ee.e2ee_repo.as_ref(), &cmd.caller).await?;
        if let Some(root_id) = cmd.thread_root_message_id {
            let root = self
                .repos
                .message_repo
                .get_by_id(root_id)
                .await?
                .ok_or(ServiceError::NotFound)?;
            if root.conversation_id != conv.id || root.thread_id.is_some() {
                return Err(ServiceError::NotFound);
            }
        }
        if let Some(source) = cmd.forwarded_from_message_id {
            self.ensure_forward_source_readable(author, cmd.server_id, source)
                .await?;
        }

        let now = Utc::now();
        let sent_at = DateTime::from_timestamp_micros(now.timestamp_micros()).unwrap_or(now);
        let (repos, e2ee_repo, scope) = self.begin_encrypted_write().await?;
        let incarnation = Self::check_envelope_against_group(
            e2ee_repo.as_ref(),
            &conv,
            &envelope,
            device.device_key_id,
            now,
        )
        .await?;
        let (franking_key_id, franking_receipt) = Self::frank(
            &e2ee.franking_keys,
            ReceiptFields {
                message_id: cmd.message_id.into(),
                conversation_id: conv.id.into(),
                sender_user_id: author.into(),
                sender_device_id: device.device_id,
                sent_at_us: sent_at_us(sent_at),
            },
            &envelope.aad.franking,
        );
        let thread = match cmd.thread_root_message_id {
            Some(root_id) => Some(Self::find_or_create_thread(&repos, conv.id, root_id).await?),
            None => None,
        };
        let message = e2ee_repo
            .insert_encrypted_message(NewEncryptedMessage {
                id: cmd.message_id,
                conversation_id: conv.id,
                sent_by: author,
                thread_id: thread.as_ref().map(|(t, _)| t.id),
                forwarded_from_message_id: cmd.forwarded_from_message_id,
                sent_at,
                envelope: MessageEncryption {
                    incarnation,
                    epoch: envelope.epoch,
                    sender_device_key_id: Some(device.device_key_id),
                    ciphertext: Some(envelope.bytes),
                    franking_commitment: envelope.aad.franking.to_vec(),
                    franking_receipt,
                    franking_key_id,
                },
            })
            .await
            .map_err(|e| match e.kind() {
                RepoErrorKind::Conflict => {
                    ServiceError::Conflict("the message id is taken".to_string())
                }
                _ => e.into(),
            })?;
        e2ee_repo
            .append_franking_revision(NewFrankingRevision {
                message_id: message.id,
                franking_commitment: envelope.aad.franking.to_vec(),
                sent_at_us: sent_at_us(sent_at),
                sender_device_id: device.device_id,
            })
            .await?;
        if let Some((thread, _)) = &thread {
            repos
                .message_thread_repo
                .touch_last_message_at(thread.id, message.sent_at)
                .await?;
        }
        repos
            .conversation_repo
            .touch_last_activity_at(conv.id, message.sent_at)
            .await?;
        let mentions =
            Self::encrypted_mentions(e2ee_repo.as_ref(), &conv, &envelope.aad, author).await?;
        if !mentions.user_ids.is_empty() {
            repos
                .message_mention_repo
                .create_mentions(message.id, &mentions.user_ids)
                .await?;
        }
        Self::commit_if_present(scope).await?;
        crate::metrics::message_created(true);
        info!(
            message_id = %message.id,
            incarnation,
            epoch = envelope.epoch,
            device_key_id = %device.device_key_id,
            "created encrypted message"
        );

        let dto = self.to_dto(message.clone(), cmd.server_id).await?;
        let member_user_ids = self.list_member_user_ids(conv.id).await?;
        if let Some((thread, true)) = &thread {
            self.observers.emit(WorkspaceEvent::ThreadCreated {
                workspace_id: cmd.server_id,
                conversation_id: conv.id,
                thread_id: thread.id,
                root_message_id: thread.first_message_id.unwrap_or(message.id),
                conversation_type: conv.conversation_type.as_str().to_string(),
                is_private: conv.is_private,
                member_user_ids: member_user_ids.clone(),
            });
        }
        self.observers.emit(WorkspaceEvent::NewMessage {
            workspace_id: cmd.server_id,
            conversation_id: conv.id,
            conversation_type: conv.conversation_type.as_str().to_string(),
            is_private: conv.is_private,
            member_user_ids: member_user_ids.clone(),
            message: MessageEventData::from(&dto),
        });
        if !mentions.user_ids.is_empty() {
            self.observers.emit(WorkspaceEvent::UserMentioned {
                workspace_id: cmd.server_id,
                conversation_id: conv.id,
                message_id: message.id,
                mentioned_user_ids: mentions.user_ids.clone(),
                author_id: author,
            });
        }
        self.notify_recipients(
            cmd.server_id,
            &conv,
            message,
            member_user_ids,
            Some(mentions),
        )
        .await;
        Ok(dto)
    }

    #[instrument(
        name = "message.edit_encrypted_message",
        skip_all,
        fields(
            actor_id = %cmd.caller.actor.user_id,
            conversation_id = %cmd.conversation_id,
            message_id = %cmd.message_id
        )
    )]
    async fn edit_encrypted_message(
        &self,
        cmd: EditEncryptedMessageCmd,
    ) -> ServiceResult<MessageDto> {
        let e2ee = self.e2ee()?;
        require_device(&cmd.caller)?;
        let author = cmd.caller.actor.user_id;
        let conv = ensure_conversation_writable(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            author,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;
        let envelope = self.decode_envelope(&cmd.envelope, cmd.message_id)?;
        let msg = self
            .repos
            .message_repo
            .get_by_id(cmd.message_id)
            .await?
            .filter(|m| m.conversation_id == conv.id)
            .ok_or(ServiceError::NotFound)?;
        let Some(current) = msg.encryption.as_ref() else {
            return Err(ServiceError::Validation(
                "this message is not encrypted".to_string(),
            ));
        };
        if msg.sent_by != author {
            return Err(ServiceError::PermissionDenied);
        }
        if msg.is_deleted {
            return Err(ServiceError::Validation(
                "cannot edit a deleted message".to_string(),
            ));
        }
        // A retry of an edit that was stored answers the message as it is.
        if current.ciphertext.as_deref() == Some(envelope.bytes.as_slice()) {
            return self.to_dto(msg, cmd.server_id).await;
        }
        let device = session_device(e2ee.e2ee_repo.as_ref(), &cmd.caller).await?;

        let now = Utc::now();
        let (repos, e2ee_repo, scope) = self.begin_encrypted_write().await?;
        let incarnation = Self::check_envelope_against_group(
            e2ee_repo.as_ref(),
            &conv,
            &envelope,
            device.device_key_id,
            now,
        )
        .await?;
        let (franking_key_id, franking_receipt) = Self::frank(
            &e2ee.franking_keys,
            ReceiptFields {
                message_id: msg.id.into(),
                conversation_id: conv.id.into(),
                sender_user_id: author.into(),
                sender_device_id: device.device_id,
                sent_at_us: sent_at_us(msg.sent_at),
            },
            &envelope.aad.franking,
        );
        let updated = e2ee_repo
            .replace_envelope(
                msg.id,
                MessageEncryption {
                    incarnation,
                    epoch: envelope.epoch,
                    sender_device_key_id: Some(device.device_key_id),
                    ciphertext: Some(envelope.bytes),
                    franking_commitment: envelope.aad.franking.to_vec(),
                    franking_receipt,
                    franking_key_id,
                },
            )
            .await?;
        // The earlier versions stay reportable.
        e2ee_repo
            .append_franking_revision(NewFrankingRevision {
                message_id: msg.id,
                franking_commitment: envelope.aad.franking.to_vec(),
                sent_at_us: sent_at_us(msg.sent_at),
                sender_device_id: device.device_id,
            })
            .await?;
        // Edits replace the mention rows and notify no one again.
        let mentions =
            Self::encrypted_mentions(e2ee_repo.as_ref(), &conv, &envelope.aad, author).await?;
        repos
            .message_mention_repo
            .delete_mentions_for_message(msg.id)
            .await?;
        if !mentions.user_ids.is_empty() {
            repos
                .message_mention_repo
                .create_mentions(msg.id, &mentions.user_ids)
                .await?;
        }
        Self::commit_if_present(scope).await?;
        info!(
            message_id = %msg.id,
            incarnation,
            epoch = envelope.epoch,
            device_key_id = %device.device_key_id,
            "edited encrypted message"
        );

        let dto = self.to_dto(updated, cmd.server_id).await?;
        let member_user_ids = self.list_member_user_ids(conv.id).await?;
        self.observers.emit(WorkspaceEvent::MessageUpdated {
            workspace_id: cmd.server_id,
            conversation_id: conv.id,
            conversation_type: conv.conversation_type.as_str().to_string(),
            is_private: conv.is_private,
            member_user_ids,
            message: MessageEventData::from(&dto),
        });
        Ok(dto)
    }

    async fn get_messages_by_ids(
        &self,
        ids: &[rim_core::message::model::MessageId],
    ) -> ServiceResult<Vec<MessageDto>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let messages_map = self.repos.message_repo.get_by_ids(ids).await?;

        let messages: Vec<_> = messages_map.into_values().collect();

        // Batch-fetch all conversations to avoid N+1
        let conv_ids: Vec<_> = messages
            .iter()
            .map(|m| m.conversation_id)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        let convs = self.repos.conversation_repo.get_by_ids(&conv_ids).await?;
        let conv_map: std::collections::HashMap<_, _> =
            convs.into_iter().map(|c| (c.id, c)).collect();

        let mut result = Vec::with_capacity(messages.len());
        for msg in messages {
            let server_id = conv_map
                .get(&msg.conversation_id)
                .map(|c| c.server_id)
                .ok_or_else(|| {
                    tracing::warn!(
                        message_id = %msg.id,
                        conversation_id = %msg.conversation_id,
                        "message references non-existent conversation"
                    );
                    ServiceError::NotFound
                })?;

            let dto = self.to_dto(msg, server_id).await?;
            result.push(dto);
        }

        Ok(result)
    }
}
