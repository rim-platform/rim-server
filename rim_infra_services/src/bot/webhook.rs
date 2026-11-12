//! Incoming webhooks: who may create, change and revoke one, the channel it is bound to, whether a
//! presented token works and what its body becomes. Running the post is the transport's, through
//! the command the bot API runs.

use chrono::Utc;
use tracing::{instrument, warn};

use rim_auth::session_token::{generate_webhook_token, hash_token, is_webhook_token_shape};
use rim_core::bot::model::{
    IncomingWebhookId, NewIncomingWebhook, WebhookPreset, WebhookTestSessionId,
};
use rim_core::conversation::model::{ConversationId, ConversationType};
use rim_core::repository::error::RepoErrorKind;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_services::bot::dto::{
    CreatedIncomingWebhookDto, IncomingWebhookDto, WebhookFormatInfoDto, WebhookPreviewDto,
    delivery_headers,
};
use rim_services::command::verb::TypedVerb;
use rim_services::conversation::command::AddConversationMembersCmd;
use rim_services::{Actor, ServiceError, ServiceResult};

use super::webhook_format::{GITHUB, Rendered, SHIPPED, WebhookFormat, parse_format, render};
use super::webhook_text::{DecodeError, plain_text};
use super::{DefaultBotService, last_use_is_stale};

const AUDIT_WEBHOOK_CREATE: &str = "bot.webhook.create";
const AUDIT_WEBHOOK_UPDATE: &str = "bot.webhook.update";
const AUDIT_WEBHOOK_REVOKE: &str = "bot.webhook.revoke";

/// The largest sample a preview takes, its headers and body together, in bytes.
const MAX_SAMPLE_BYTES: usize = 256 * 1024;

/// The refusal of a format on a webhook of the GitHub preset.
const FORMAT_NEEDS_PLAIN: &str = "A webhook with its own format has the preset PLAIN.";

/// The refusal of a change to a revoked webhook.
const REVOKED_WEBHOOK: &str = "A revoked webhook cannot be changed.";

/// The refusal of a second webhook on a channel.
const CHANNEL_HAS_ONE: &str = "This channel already has a webhook. Revoke it first.";

/// The refusal of a webhook for a bot that is deactivated.
const DEACTIVATED_BOT: &str = "A deactivated bot cannot have a webhook.";

/// The text a delivery of `headers` and `body` to `webhook` becomes, or `None` when there is
/// nothing to post: its own format, the shipped GitHub format for the GitHub preset, or the plain
/// `{"text"}` body.
///
/// # Errors
/// `Validation` for a body the webhook cannot read; `Internal` for a format that doesn't pass its
/// checks, logged with the webhook's id alone.
pub(super) fn text_of(
    webhook: &IncomingWebhookDto,
    headers: &[(String, String)],
    body: &[u8],
) -> ServiceResult<Option<String>> {
    let format = match (&webhook.template, webhook.preset) {
        (None, WebhookPreset::Plain) => return plain_text(body).map_err(refused_body),
        (None, WebhookPreset::Github) => GITHUB.parsed(),
        (Some(stored), _) => parse_format(stored),
    }
    .map_err(|_| {
        warn!(webhook_id = %webhook.id, "a webhook's format does not pass its checks");
        ServiceError::internal(anyhow::anyhow!(
            "a webhook's format does not pass its checks"
        ))
    })?;
    render(&format, headers, body)
        .map(|rendered| rendered.message())
        .map_err(refused_body)
}

fn refused_body(DecodeError(reason): DecodeError) -> ServiceError {
    ServiceError::Validation(reason.to_string())
}

/// `template` checked for a webhook of `preset`: `None` when there is none, which any preset
/// takes, and a format only with the preset `Plain`.
///
/// # Errors
/// `Validation` for a format with another preset and for one that is refused, naming the rule.
fn checked_template(
    preset: WebhookPreset,
    template: Option<&serde_json::Value>,
) -> ServiceResult<Option<WebhookFormat>> {
    let Some(template) = template else {
        return Ok(None);
    };
    if preset != WebhookPreset::Plain {
        return Err(ServiceError::Validation(FORMAT_NEEDS_PLAIN.to_string()));
    }
    parse_format(template)
        .map(Some)
        .map_err(|e| ServiceError::Validation(e.to_string()))
}

/// The ready-made formats, as a client lists them.
///
/// # Errors
/// `Internal` for a shipped format that isn't JSON, which the tests rule out.
pub(super) fn shipped_formats() -> ServiceResult<Vec<WebhookFormatInfoDto>> {
    SHIPPED
        .iter()
        .map(|shipped| {
            Ok(WebhookFormatInfoDto {
                key: shipped.key.to_string(),
                name: shipped.name.to_string(),
                setup_note: shipped.setup_note.to_string(),
                format: shipped.json().map_err(ServiceError::internal)?,
                sample_headers: shipped
                    .sample_headers
                    .iter()
                    .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                    .collect(),
                sample_body: shipped.sample_body.to_string(),
            })
        })
        .collect()
}

impl DefaultBotService {
    #[instrument(
        name = "bot.webhook.create",
        skip_all,
        fields(conversation_id = %conversation_id, bot_user_id = %bot_user_id)
    )]
    pub(super) async fn create_incoming_webhook(
        &self,
        actor: Actor,
        conversation_id: ConversationId,
        bot_user_id: UserId,
        preset: WebhookPreset,
        template: Option<serde_json::Value>,
        test_session: Option<WebhookTestSessionId>,
    ) -> ServiceResult<CreatedIncomingWebhookDto> {
        self.require_feature()?;
        let conversation = self
            .repos
            .conversation_repo
            .get_by_id(conversation_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let server_id = conversation.server_id;
        self.ensure_moderator(server_id, actor.user_id).await?;
        let bot = self
            .repos
            .bot_repo
            .find_bot(bot_user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if bot.server_id != server_id {
            return Err(ServiceError::Validation(
                "The bot is not of this workspace.".to_string(),
            ));
        }
        if bot.deactivated_at.is_some() {
            return Err(ServiceError::Validation(DEACTIVATED_BOT.to_string()));
        }
        if conversation.conversation_type != ConversationType::Channel
            || conversation.is_encrypted()
        {
            return Err(ServiceError::Validation(
                "A webhook can only be on a channel.".to_string(),
            ));
        }
        if conversation.is_archived() {
            return Err(ServiceError::Validation(
                "An archived channel cannot have a webhook.".to_string(),
            ));
        }
        // Posting into a private channel takes the standing that adding a bot to it takes.
        if conversation.is_private
            && !self
                .repos
                .conversation_member_repo
                .is_member(conversation_id, actor.user_id)
                .await?
        {
            return Err(ServiceError::PermissionDenied);
        }
        checked_template(preset, template.as_ref())?;
        let keep = self.session_to_keep(actor.user_id, server_id, test_session)?;
        if self
            .repos
            .bot_repo
            .find_active_webhook(conversation_id)
            .await?
            .is_some()
        {
            return Err(ServiceError::Conflict(CHANNEL_HAS_ONE.to_string()));
        }
        if !self
            .repos
            .conversation_member_repo
            .is_member(conversation_id, bot_user_id)
            .await?
        {
            self.conversations
                .add_conversation_members(AddConversationMembersCmd {
                    actor,
                    server_id,
                    conversation_id,
                    user_ids: vec![bot_user_id],
                })
                .await?;
        }

        let (scope, repos) = self.transaction().await?;
        repos.bot_repo.lock_bot(bot_user_id).await?;
        // Read under the lock: a deactivation that committed first is seen here, and one that
        // comes later revokes this row with the bot's others.
        let current = repos
            .bot_repo
            .find_bot(bot_user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if current.deactivated_at.is_some() {
            return Err(ServiceError::Validation(DEACTIVATED_BOT.to_string()));
        }
        // A kept session's token is known here by its hash alone, so the answer's token is empty.
        let kept = self.take_session(keep);
        let (token, token_hash) = match &kept {
            Some((_, hash)) => (String::new(), hash.clone()),
            None => {
                let token = generate_webhook_token();
                let hash = hash_token(&token);
                (token, hash)
            }
        };
        let created = repos
            .bot_repo
            .create_webhook(NewIncomingWebhook {
                server_id,
                conversation_id,
                bot_user_id,
                preset,
                template,
                token_hash,
                created_by: actor.user_id,
            })
            .await
            .map_err(|e| match e.kind() {
                // Two creations at once: the partial unique index decides between them.
                RepoErrorKind::Conflict => ServiceError::Conflict(CHANNEL_HAS_ONE.to_string()),
                _ => ServiceError::from(e),
            })?;
        Self::commit(scope).await?;
        self.audit(
            actor.user_id,
            server_id,
            AUDIT_WEBHOOK_CREATE,
            serde_json::json!({
                "webhook_id": created.id.to_string(),
                "conversation_id": conversation_id.to_string(),
                "bot_user_id": bot_user_id.to_string(),
            }),
            TypedVerb::Write,
        )
        .await;
        if let Some((session, _)) = &kept {
            self.audit_kept_session(actor.user_id, session).await;
        }
        Ok(CreatedIncomingWebhookDto {
            webhook: created.into(),
            token,
            kept_test_url: kept.is_some(),
        })
    }

    #[instrument(name = "bot.webhook.update", skip_all, fields(webhook_id = %id))]
    pub(super) async fn update_incoming_webhook_format(
        &self,
        actor: Actor,
        id: IncomingWebhookId,
        preset: WebhookPreset,
        template: Option<serde_json::Value>,
    ) -> ServiceResult<IncomingWebhookDto> {
        self.require_feature()?;
        let webhook = self
            .repos
            .bot_repo
            .find_webhook(id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        // The same answer as an unknown id, so an outsider learns nothing about a webhook.
        self.ensure_moderator(webhook.server_id, actor.user_id)
            .await
            .map_err(|e| match e {
                ServiceError::PermissionDenied => ServiceError::NotFound,
                other => other,
            })?;
        let conversation = self
            .repos
            .conversation_repo
            .get_by_id(webhook.conversation_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        // Changing what posts into a private channel takes the standing that creating it takes.
        if conversation.is_private
            && !self
                .repos
                .conversation_member_repo
                .is_member(webhook.conversation_id, actor.user_id)
                .await?
        {
            return Err(ServiceError::PermissionDenied);
        }
        if webhook.revoked_at.is_some() {
            return Err(ServiceError::Validation(REVOKED_WEBHOOK.to_string()));
        }
        checked_template(preset, template.as_ref())?;
        let updated = self
            .repos
            .bot_repo
            .update_webhook_format(id, preset, template)
            .await?
            .ok_or_else(|| ServiceError::Validation(REVOKED_WEBHOOK.to_string()))?;
        self.audit(
            actor.user_id,
            updated.server_id,
            AUDIT_WEBHOOK_UPDATE,
            serde_json::json!({
                "webhook_id": updated.id.to_string(),
                "conversation_id": updated.conversation_id.to_string(),
                "bot_user_id": updated.bot_user_id.to_string(),
            }),
            TypedVerb::Write,
        )
        .await;
        Ok(updated.into())
    }

    pub(super) async fn preview_incoming_webhook_format(
        &self,
        actor: Actor,
        server_id: ServerId,
        template: serde_json::Value,
        headers: Vec<(String, String)>,
        body: String,
    ) -> ServiceResult<WebhookPreviewDto> {
        self.require_feature()?;
        self.ensure_moderator(server_id, actor.user_id).await?;
        let sample_bytes = body.len()
            + headers
                .iter()
                .map(|(name, value)| name.len() + value.len())
                .sum::<usize>();
        if sample_bytes > MAX_SAMPLE_BYTES {
            return Err(ServiceError::Validation(
                "The sample is over 256 KiB.".to_string(),
            ));
        }
        let format = match parse_format(&template) {
            Ok(format) => format,
            Err(refused) => {
                return Ok(WebhookPreviewDto::FormatRefused {
                    rule: refused.rule,
                    reason: refused.reason,
                });
            }
        };
        let headers = delivery_headers(
            headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        );
        Ok(match render(&format, &headers, body.as_bytes()) {
            Ok(Rendered::Post { rule, text, link }) => {
                WebhookPreviewDto::Posts { rule, text, link }
            }
            Ok(Rendered::NoRule) => WebhookPreviewDto::NoRule,
            Ok(Rendered::NoValue { rule, placeholder }) => {
                WebhookPreviewDto::NoValue { rule, placeholder }
            }
            Err(DecodeError(reason)) => WebhookPreviewDto::BodyRefused {
                reason: reason.to_string(),
            },
        })
    }

    #[instrument(name = "bot.webhook.revoke", skip_all, fields(webhook_id = %id))]
    pub(super) async fn revoke_incoming_webhook(
        &self,
        actor: Actor,
        id: IncomingWebhookId,
    ) -> ServiceResult<IncomingWebhookDto> {
        self.require_feature()?;
        let webhook = self
            .repos
            .bot_repo
            .find_webhook(id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        // The same answer as an unknown id, so an outsider learns nothing about a webhook.
        self.ensure_moderator(webhook.server_id, actor.user_id)
            .await
            .map_err(|e| match e {
                ServiceError::PermissionDenied => ServiceError::NotFound,
                other => other,
            })?;
        if webhook.revoked_at.is_some() {
            return Ok(webhook.into());
        }
        let revoked = self
            .repos
            .bot_repo
            .revoke_webhook(id, Utc::now())
            .await?
            .ok_or(ServiceError::NotFound)?;
        self.audit(
            actor.user_id,
            revoked.server_id,
            AUDIT_WEBHOOK_REVOKE,
            serde_json::json!({
                "webhook_id": revoked.id.to_string(),
                "conversation_id": revoked.conversation_id.to_string(),
                "bot_user_id": revoked.bot_user_id.to_string(),
            }),
            TypedVerb::Delete,
        )
        .await;
        self.end_sessions_of(actor.user_id, id).await;
        Ok(revoked.into())
    }

    pub(super) async fn list_incoming_webhooks(
        &self,
        actor: Actor,
        server_id: ServerId,
    ) -> ServiceResult<Vec<IncomingWebhookDto>> {
        self.require_feature()?;
        self.ensure_moderator(server_id, actor.user_id).await?;
        let webhooks = self.repos.bot_repo.list_webhooks(server_id).await?;
        Ok(webhooks.into_iter().map(Into::into).collect())
    }

    pub(super) async fn resolve_incoming_webhook(
        &self,
        raw: &str,
    ) -> ServiceResult<Option<IncomingWebhookDto>> {
        if !is_webhook_token_shape(raw) || self.require_feature().is_err() {
            return Ok(None);
        }
        let Some(holder) = self
            .repos
            .bot_repo
            .find_webhook_holder(&hash_token(raw))
            .await?
        else {
            return Ok(None);
        };
        let webhook = holder.webhook;
        if webhook.revoked_at.is_some() || holder.bot_deactivated_at.is_some() {
            return Ok(None);
        }
        if !self
            .bot_is_in_channel(
                webhook.server_id,
                webhook.bot_user_id,
                webhook.conversation_id,
            )
            .await?
        {
            return Ok(None);
        }
        let now = Utc::now();
        if last_use_is_stale(webhook.last_used_at, now)
            && let Err(e) = self.repos.bot_repo.touch_webhook(webhook.id, now).await
        {
            warn!(webhook_id = %webhook.id, error = %e, "could not record a webhook's use");
        }
        Ok(Some(webhook.into()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::DateTime;
    use rim_auth::session_token::is_webhook_token_shape;
    use rim_core::bot::model::{Bot, IncomingWebhook, IncomingWebhookHolder};
    use rim_core::bot::repository::MockBotRepository;
    use rim_core::conversation::model::{Conversation, PostingPolicy};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::message_mention::repository::MockMessageMentionRepository;
    use rim_core::moderation::repository::MockModerationRepository;
    use rim_core::repository::error::{RepoError, RepoResult};
    use rim_core::server_feature::model::ServerFeature;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_services::audit::dto::{AuditLogEntryDto, AuditLogPage, ListAuditLogQuery};
    use rim_services::audit::service::AuditLogService;
    use rim_services::bot::service::BotService;
    use rim_services::bot::test_session::{
        InMemoryTestSessionStore, MAX_TEST_BODY_BYTES, MAX_TEST_SESSIONS_PER_WORKSPACE,
        NewTestSession, TestBody, WebhookTestSessionStore,
    };
    use rim_services::conversation::service::MockConversationService;
    use rim_services::events::{Broadcaster, Observers, WorkspaceEvent};
    use rim_services::moderation::service::MockModerationService;

    use crate::bot::BotRepos;
    use crate::feature_gate::InstanceFeatureGate;

    use super::*;

    const GOOD_TOKEN: &str = "rimhook_abcdefghijklmnopqrstuvwxyz0123456789_-ABCDE";

    #[derive(Default)]
    struct RecordingAudit(Mutex<Vec<AuditLogEntryDto>>);

    #[async_trait]
    impl AuditLogService for RecordingAudit {
        async fn write_entry(&self, entry: AuditLogEntryDto) -> ServiceResult<()> {
            self.0.lock().expect("lock").push(entry);
            Ok(())
        }

        async fn list_entries(
            &self,
            _caller: Actor,
            _query: ListAuditLogQuery,
        ) -> ServiceResult<AuditLogPage> {
            Err(ServiceError::NotFound)
        }
    }

    struct NoBroadcast;

    impl Broadcaster for NoBroadcast {
        fn broadcast(&self, _event: WorkspaceEvent) {}
    }

    /// The mocks a test sets up. A call nothing expected panics, so a gate that stops a call early
    /// shows in what a test leaves out.
    struct Fx {
        bots: MockBotRepository,
        members: MockServerMemberRepository,
        conversation_repo: MockConversationRepository,
        conversation_members: MockConversationMemberRepository,
        conversations: MockConversationService,
        disabled: Vec<ServerFeature>,
        audit: Arc<RecordingAudit>,
        server_id: ServerId,
        channel: ConversationId,
        bot: UserId,
    }

    impl Fx {
        fn new() -> Self {
            Self {
                bots: MockBotRepository::new(),
                members: MockServerMemberRepository::new(),
                conversation_repo: MockConversationRepository::new(),
                conversation_members: MockConversationMemberRepository::new(),
                conversations: MockConversationService::new(),
                disabled: Vec::new(),
                audit: Arc::default(),
                server_id: ServerId::new(),
                channel: ConversationId::new(),
                bot: UserId::new(),
            }
        }

        fn with_role(mut self, role: Option<ServerMemberRole>) -> Self {
            self.members
                .expect_get_member()
                .returning(move |server_id, user_id| {
                    Ok(role.map(|member_role| ServerMember {
                        id: ServerMemberId::new(),
                        server_id,
                        user_id,
                        member_role,
                        joined_at: Utc::now(),
                    }))
                });
            self
        }

        /// The conversation looks like `shape` says of a public plaintext channel.
        fn with_conversation(mut self, shape: impl Fn(&mut Conversation) + Send + 'static) -> Self {
            let (server_id, channel) = (self.server_id, self.channel);
            self.conversation_repo
                .expect_get_by_id()
                .returning(move |_| {
                    let mut conversation = a_channel(server_id, channel);
                    shape(&mut conversation);
                    ok(Some(conversation))
                });
            self
        }

        fn with_bot(mut self) -> Self {
            let (server_id, bot) = (self.server_id, self.bot);
            self.bots
                .expect_find_bot()
                .returning(move |_| ok(Some(a_bot(server_id, bot))));
            self
        }

        /// `who` says who is in the channel.
        fn with_channel_members(mut self, who: impl Fn(UserId) -> bool + Send + 'static) -> Self {
            self.conversation_members
                .expect_is_member()
                .returning(move |_, user_id| ok(who(user_id)));
            self
        }

        fn with_no_active_webhook(mut self) -> Self {
            self.bots
                .expect_find_active_webhook()
                .returning(|_| ok(None));
            self
        }

        fn with_insert(mut self) -> Self {
            self.bots.expect_lock_bot().returning(|_| ok(true));
            self.bots.expect_create_webhook().returning(|new| {
                ok(IncomingWebhook {
                    id: IncomingWebhookId::new(),
                    server_id: new.server_id,
                    conversation_id: new.conversation_id,
                    bot_user_id: new.bot_user_id,
                    preset: new.preset,
                    template: new.template,
                    created_at: Utc::now(),
                    last_used_at: None,
                    revoked_at: None,
                })
            });
            self
        }

        fn build(self) -> (DefaultBotService, Arc<RecordingAudit>) {
            let audit = Arc::clone(&self.audit);
            let repos = BotRepos {
                bot_repo: Arc::new(self.bots),
                server_member_repo: Arc::new(self.members),
                conversation_repo: Arc::new(self.conversation_repo),
                conversation_member_repo: Arc::new(self.conversation_members),
                moderation_repo: Arc::new(MockModerationRepository::new()),
                message_mention_repo: Arc::new(MockMessageMentionRepository::new()),
            };
            let service = DefaultBotService::new(
                repos,
                Observers::new(Arc::new(NoBroadcast)),
                Arc::new(InstanceFeatureGate::new(&self.disabled, &[])),
                Arc::new(MockModerationService::new()),
                Arc::new(self.conversations),
            )
            .with_audit_log_service(self.audit);
            (service, audit)
        }
    }

    fn ok<T: Send + 'static>(
        value: T,
    ) -> std::pin::Pin<Box<dyn Future<Output = RepoResult<T>> + Send>> {
        Box::pin(async move { Ok(value) })
    }

    fn a_channel(server_id: ServerId, id: ConversationId) -> Conversation {
        Conversation {
            id,
            server_id,
            conversation_type: ConversationType::Channel,
            title: Some("ops".to_string()),
            topic: None,
            is_private: false,
            archived_at: None,
            created_by: None,
            slow_mode_seconds: 0,
            posting_policy: PostingPolicy::Everyone,
            posting_allows_threads: true,
            encrypted_at: None,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
        }
    }

    fn a_bot(server_id: ServerId, user_id: UserId) -> Bot {
        Bot {
            user_id,
            server_id,
            name: "Helper".to_string(),
            run_by: "Ada".to_string(),
            reads_go_to: "Nowhere".to_string(),
            created_by: UserId::new(),
            added_by_name: None,
            created_at: Utc::now(),
            deactivated_at: None,
            member_joins_since: None,
        }
    }

    fn a_webhook(fx: &Fx) -> IncomingWebhook {
        IncomingWebhook {
            id: IncomingWebhookId::new(),
            server_id: fx.server_id,
            conversation_id: fx.channel,
            bot_user_id: fx.bot,
            preset: WebhookPreset::Plain,
            template: None,
            created_at: Utc::now(),
            last_used_at: None,
            revoked_at: None,
        }
    }

    fn actor() -> Actor {
        Actor::new(UserId::new())
    }

    async fn create(
        svc: &DefaultBotService,
        channel: ConversationId,
        bot: UserId,
    ) -> ServiceResult<CreatedIncomingWebhookDto> {
        BotService::create_webhook(
            svc,
            actor(),
            channel,
            bot,
            WebhookPreset::Github,
            None,
            None,
        )
        .await
    }

    fn assert_validation<T: std::fmt::Debug>(result: ServiceResult<T>) {
        assert!(
            matches!(result, Err(ServiceError::Validation(_))),
            "{result:?}"
        );
    }

    // ── create ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_bot_not_in_the_channel_goes_through_the_guarded_path_and_its_refusal_stops_it() {
        let fx = Fx::new();
        let (channel, bot, server_id) = (fx.channel, fx.bot, fx.server_id);
        let mut fx = fx
            .with_role(Some(ServerMemberRole::Moderator))
            .with_conversation(|_| {})
            .with_bot()
            .with_channel_members(|_| false)
            .with_no_active_webhook();
        fx.conversations
            .expect_add_conversation_members()
            .withf(move |cmd| {
                cmd.server_id == server_id
                    && cmd.conversation_id == channel
                    && cmd.user_ids == vec![bot]
            })
            .times(1)
            .returning(|_| Box::pin(async { Err(ServiceError::PermissionDenied) }));
        // No insert is expected: a refused add writes no row.
        let (svc, _) = fx.build();
        assert!(matches!(
            create(&svc, channel, bot).await,
            Err(ServiceError::PermissionDenied)
        ));
    }

    #[tokio::test]
    async fn a_bot_already_in_the_channel_is_not_added_again_and_nothing_secret_is_audited() {
        let fx = Fx::new();
        let (channel, bot) = (fx.channel, fx.bot);
        let (svc, audit) = fx
            .with_role(Some(ServerMemberRole::Owner))
            .with_conversation(|_| {})
            .with_bot()
            .with_channel_members(|_| true)
            .with_no_active_webhook()
            .with_insert()
            .build();

        let created = create(&svc, channel, bot).await.expect("created");

        assert!(is_webhook_token_shape(&created.token), "{}", created.token);
        assert!(!format!("{created:?}").contains(&created.token));
        assert_eq!(created.webhook.preset, WebhookPreset::Github);
        let entries = audit.0.lock().expect("lock");
        let [entry] = entries.as_slice() else {
            panic!("{entries:?}");
        };
        assert_eq!(entry.command_id, AUDIT_WEBHOOK_CREATE);
        let args = entry.args_redacted.to_string();
        assert!(!args.contains(&created.token));
        assert!(!args.contains(&hash_token(&created.token)));
        assert_eq!(
            entry.args_redacted,
            serde_json::json!({
                "webhook_id": created.webhook.id.to_string(),
                "conversation_id": channel.to_string(),
                "bot_user_id": bot.to_string(),
            })
        );
    }

    #[tokio::test]
    async fn the_stored_hash_is_the_tokens_sha256() {
        let hash = Arc::new(Mutex::new(String::new()));
        let seen = Arc::clone(&hash);
        let fx = Fx::new();
        let (channel, bot) = (fx.channel, fx.bot);
        let mut fx = fx
            .with_role(Some(ServerMemberRole::Owner))
            .with_conversation(|_| {})
            .with_bot()
            .with_channel_members(|_| true)
            .with_no_active_webhook();
        let row = a_webhook(&fx);
        fx.bots.expect_lock_bot().returning(|_| ok(true));
        fx.bots.expect_create_webhook().returning(move |new| {
            *seen.lock().expect("lock") = new.token_hash.clone();
            ok(row.clone())
        });
        let (svc, _) = fx.build();
        let created = create(&svc, channel, bot).await.expect("created");
        assert_eq!(*hash.lock().expect("lock"), hash_token(&created.token));
    }

    #[tokio::test]
    async fn anyone_but_a_moderator_is_refused_before_anything_else() {
        for role in [
            Some(ServerMemberRole::Member),
            Some(ServerMemberRole::Guest),
            None,
        ] {
            let fx = Fx::new();
            let (channel, bot) = (fx.channel, fx.bot);
            let (svc, _) = fx.with_role(role).with_conversation(|_| {}).build();
            assert!(matches!(
                create(&svc, channel, bot).await,
                Err(ServiceError::PermissionDenied)
            ));
        }
    }

    #[tokio::test]
    async fn only_a_plaintext_channel_that_is_not_archived_can_have_one() {
        for shape in [
            (|c: &mut Conversation| c.conversation_type = ConversationType::Direct)
                as fn(&mut Conversation),
            |c| c.conversation_type = ConversationType::Group,
            |c| c.encrypted_at = Some(Utc::now()),
            |c| c.archived_at = Some(Utc::now()),
        ] {
            let fx = Fx::new();
            let (channel, bot) = (fx.channel, fx.bot);
            let (svc, _) = fx
                .with_role(Some(ServerMemberRole::Owner))
                .with_conversation(shape)
                .with_bot()
                .build();
            assert_validation(create(&svc, channel, bot).await);
        }
    }

    #[tokio::test]
    async fn the_bot_must_be_active_and_of_the_workspace() {
        let fx = Fx::new();
        let (channel, bot) = (fx.channel, fx.bot);
        let mut fx = fx
            .with_role(Some(ServerMemberRole::Owner))
            .with_conversation(|_| {});
        fx.bots
            .expect_find_bot()
            .returning(move |_| ok(Some(a_bot(ServerId::new(), bot))));
        assert_validation(create(&fx.build().0, channel, bot).await);

        let fx = Fx::new();
        let (channel, bot, server_id) = (fx.channel, fx.bot, fx.server_id);
        let mut fx = fx
            .with_role(Some(ServerMemberRole::Owner))
            .with_conversation(|_| {});
        fx.bots.expect_find_bot().returning(move |_| {
            ok(Some(Bot {
                deactivated_at: Some(Utc::now()),
                ..a_bot(server_id, bot)
            }))
        });
        assert_validation(create(&fx.build().0, channel, bot).await);

        let fx = Fx::new();
        let (channel, bot) = (fx.channel, fx.bot);
        let mut fx = fx
            .with_role(Some(ServerMemberRole::Owner))
            .with_conversation(|_| {});
        fx.bots.expect_find_bot().returning(|_| ok(None));
        assert!(matches!(
            create(&fx.build().0, channel, bot).await,
            Err(ServiceError::NotFound)
        ));
    }

    #[tokio::test]
    async fn a_deactivation_that_commits_first_is_seen_under_the_lock_and_no_row_is_written() {
        let fx = Fx::new();
        let (channel, bot, server_id) = (fx.channel, fx.bot, fx.server_id);
        let mut fx = fx
            .with_role(Some(ServerMemberRole::Owner))
            .with_conversation(|_| {})
            .with_channel_members(|_| true)
            .with_no_active_webhook();
        let mut reads = 0;
        fx.bots.expect_find_bot().returning(move |_| {
            reads += 1;
            ok(Some(Bot {
                deactivated_at: (reads > 1).then(Utc::now),
                ..a_bot(server_id, bot)
            }))
        });
        fx.bots.expect_lock_bot().times(1).returning(|_| ok(true));
        assert_validation(create(&fx.build().0, channel, bot).await);
    }

    #[tokio::test]
    async fn a_private_channel_takes_a_moderator_who_is_in_it_even_with_the_bot_there() {
        let fx = Fx::new();
        let (channel, bot) = (fx.channel, fx.bot);
        let (svc, _) = fx
            .with_role(Some(ServerMemberRole::Owner))
            .with_conversation(|c| c.is_private = true)
            .with_bot()
            .with_channel_members(move |user| user == bot)
            .build();
        assert!(matches!(
            create(&svc, channel, bot).await,
            Err(ServiceError::PermissionDenied)
        ));
    }

    #[tokio::test]
    async fn a_second_webhook_is_a_conflict_found_or_lost_in_a_race() {
        let fx = Fx::new();
        let (channel, bot) = (fx.channel, fx.bot);
        let mut fx = fx
            .with_role(Some(ServerMemberRole::Owner))
            .with_conversation(|_| {})
            .with_bot()
            .with_channel_members(|_| true);
        let existing = a_webhook(&fx);
        fx.bots
            .expect_find_active_webhook()
            .returning(move |_| ok(Some(existing.clone())));
        let refused = create(&fx.build().0, channel, bot).await;
        assert!(
            matches!(&refused, Err(ServiceError::Conflict(m)) if m == CHANNEL_HAS_ONE),
            "{refused:?}"
        );

        let fx = Fx::new();
        let (channel, bot) = (fx.channel, fx.bot);
        let mut fx = fx
            .with_role(Some(ServerMemberRole::Owner))
            .with_conversation(|_| {})
            .with_bot()
            .with_channel_members(|_| true)
            .with_no_active_webhook();
        fx.bots.expect_lock_bot().returning(|_| ok(true));
        fx.bots
            .expect_create_webhook()
            .returning(|_| Box::pin(async { Err(RepoError::new(RepoErrorKind::Conflict)) }));
        let refused = create(&fx.build().0, channel, bot).await;
        assert!(
            matches!(&refused, Err(ServiceError::Conflict(m)) if m == CHANNEL_HAS_ONE),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn every_call_with_an_actor_refuses_while_the_feature_is_off() {
        let mut fx = Fx::new();
        fx.disabled.push(ServerFeature::Bots);
        let (channel, bot, server_id) = (fx.channel, fx.bot, fx.server_id);
        let (svc, _) = fx.build();
        let off =
            |e: &ServiceError| matches!(e, ServiceError::FeatureDisabled(ServerFeature::Bots));
        assert!(off(&create(&svc, channel, bot).await.expect_err("off")));
        assert!(off(&BotService::revoke_webhook(
            &svc,
            actor(),
            IncomingWebhookId::new()
        )
        .await
        .expect_err("off")));
        assert!(off(&BotService::list_webhooks(&svc, actor(), server_id)
            .await
            .expect_err("off")));
    }

    // ── revoke and list ──────────────────────────────────────────────────

    #[tokio::test]
    async fn revoking_answers_not_found_outside_the_actors_moderation() {
        for role in [None, Some(ServerMemberRole::Member)] {
            let mut fx = Fx::new().with_role(role);
            let webhook = a_webhook(&fx);
            let id = webhook.id;
            fx.bots
                .expect_find_webhook()
                .returning(move |_| ok(Some(webhook.clone())));
            let (svc, _) = fx.build();
            assert!(matches!(
                BotService::revoke_webhook(&svc, actor(), id).await,
                Err(ServiceError::NotFound)
            ));
        }

        let mut fx = Fx::new();
        fx.bots.expect_find_webhook().returning(|_| ok(None));
        let (svc, _) = fx.build();
        assert!(matches!(
            BotService::revoke_webhook(&svc, actor(), IncomingWebhookId::new()).await,
            Err(ServiceError::NotFound)
        ));
    }

    #[tokio::test]
    async fn revoking_is_idempotent_and_audited_once() {
        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Moderator));
        let webhook = a_webhook(&fx);
        let id = webhook.id;
        let revoked = IncomingWebhook {
            revoked_at: Some(Utc::now()),
            ..webhook.clone()
        };
        let already = revoked.clone();
        fx.bots
            .expect_find_webhook()
            .returning(move |_| ok(Some(webhook.clone())));
        fx.bots
            .expect_revoke_webhook()
            .times(1)
            .returning(move |_, _| ok(Some(revoked.clone())));
        let (svc, audit) = fx.build();
        let dto = BotService::revoke_webhook(&svc, actor(), id)
            .await
            .expect("revoked");
        assert!(dto.revoked_at.is_some());
        {
            let entries = audit.0.lock().expect("lock");
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].command_id, AUDIT_WEBHOOK_REVOKE);
        }

        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Owner));
        fx.bots
            .expect_find_webhook()
            .returning(move |_| ok(Some(already.clone())));
        let (svc, audit) = fx.build();
        BotService::revoke_webhook(&svc, actor(), id)
            .await
            .expect("again");
        assert!(audit.0.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn only_a_moderator_lists_them() {
        let server_id = ServerId::new();
        for role in [
            None,
            Some(ServerMemberRole::Guest),
            Some(ServerMemberRole::Member),
        ] {
            let (svc, _) = Fx::new().with_role(role).build();
            assert!(matches!(
                BotService::list_webhooks(&svc, actor(), server_id).await,
                Err(ServiceError::PermissionDenied)
            ));
        }
        let mut fx = Fx::new().with_role(Some(ServerMemberRole::Moderator));
        let webhook = a_webhook(&fx);
        fx.bots
            .expect_list_webhooks()
            .returning(move |_| ok(vec![webhook.clone()]));
        let (svc, _) = fx.build();
        assert_eq!(
            BotService::list_webhooks(&svc, actor(), server_id)
                .await
                .expect("listed")
                .len(),
            1
        );
    }

    // ── resolve ──────────────────────────────────────────────────────────

    /// A service whose lookup finds `found`, of a bot in the channel when `in_channel`.
    fn resolving(
        mut fx: Fx,
        last_used_at: Option<DateTime<Utc>>,
        revoked_at: Option<DateTime<Utc>>,
        bot_deactivated_at: Option<DateTime<Utc>>,
        in_channel: bool,
    ) -> Fx {
        let holder = IncomingWebhookHolder {
            webhook: IncomingWebhook {
                last_used_at,
                revoked_at,
                ..a_webhook(&fx)
            },
            bot_deactivated_at,
        };
        fx.bots
            .expect_find_webhook_holder()
            .returning(move |_| ok(Some(holder.clone())));
        fx.with_conversation(|_| {})
            .with_channel_members(move |_| in_channel)
    }

    #[tokio::test]
    async fn a_token_of_the_wrong_shape_is_not_even_looked_up() {
        let (svc, _) = Fx::new().build();
        for raw in [
            "",
            "rimhook_",
            "rimhook_short",
            "rimbot_abcdefghijklmnopqrstuvwxyz0123456789_-ABCDE",
            "rimhook_abcdefghijklmnopqrstuvwxyz0123456789_-ABCD!",
            "rimhook_abcdefghijklmnopqrstuvwxyz0123456789_-ABCDEF",
        ] {
            assert_eq!(
                BotService::resolve_webhook(&svc, raw).await.expect("ok"),
                None,
                "{raw}"
            );
        }
    }

    #[tokio::test]
    async fn a_token_that_does_not_work_looks_the_same_whatever_the_cause() {
        let mut unknown = Fx::new();
        unknown
            .bots
            .expect_find_webhook_holder()
            .returning(|_| ok(None));
        let mut off = Fx::new();
        off.disabled.push(ServerFeature::Bots);
        let revoked = resolving(Fx::new(), None, Some(Utc::now()), None, true);
        let deactivated = resolving(Fx::new(), None, None, Some(Utc::now()), true);
        let left_channel = resolving(Fx::new(), None, None, None, false);
        for fx in [unknown, off, revoked, deactivated, left_channel] {
            let (svc, _) = fx.build();
            assert_eq!(
                BotService::resolve_webhook(&svc, GOOD_TOKEN)
                    .await
                    .expect("ok"),
                None
            );
        }
    }

    #[tokio::test]
    async fn a_working_token_records_its_use_at_most_once_a_minute() {
        let mut fx = resolving(Fx::new(), None, None, None, true);
        fx.bots
            .expect_touch_webhook()
            .times(1)
            .returning(|_, _| ok(()));
        let (svc, _) = fx.build();
        assert!(
            BotService::resolve_webhook(&svc, GOOD_TOKEN)
                .await
                .expect("ok")
                .is_some()
        );

        let (svc, _) = resolving(Fx::new(), Some(Utc::now()), None, None, true).build();
        assert!(
            BotService::resolve_webhook(&svc, GOOD_TOKEN)
                .await
                .expect("ok")
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_failed_use_record_does_not_stop_the_post() {
        let mut fx = resolving(Fx::new(), None, None, None, true);
        fx.bots
            .expect_touch_webhook()
            .returning(|_, _| Box::pin(async { Err(RepoError::new(RepoErrorKind::Unavailable)) }));
        let (svc, _) = fx.build();
        assert!(
            BotService::resolve_webhook(&svc, GOOD_TOKEN)
                .await
                .expect("ok")
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_lookup_goes_by_the_hash_and_a_failed_one_is_an_error() {
        let mut fx = Fx::new();
        let expected = hash_token(GOOD_TOKEN);
        fx.bots
            .expect_find_webhook_holder()
            .withf(move |hash| hash == expected)
            .returning(|_| Box::pin(async { Err(RepoError::new(RepoErrorKind::Unavailable)) }));
        let (svc, _) = fx.build();
        assert!(BotService::resolve_webhook(&svc, GOOD_TOKEN).await.is_err());
    }

    // ── formats ──────────────────────────────────────────────────────────

    fn a_format() -> serde_json::Value {
        serde_json::json!({ "version": 1, "rules": [
            { "when": [{ "field": "status", "equals": "failed" }], "post": "Backup failed on {host}" },
        ]})
    }

    #[tokio::test]
    async fn a_format_is_stored_with_the_plain_preset_and_checked_before_anything_is_written() {
        let fx = Fx::new();
        let (channel, bot) = (fx.channel, fx.bot);
        let fx = fx
            .with_role(Some(ServerMemberRole::Moderator))
            .with_conversation(|_| {})
            .with_bot()
            .with_channel_members(|_| true)
            .with_no_active_webhook()
            .with_insert();
        let (svc, _) = fx.build();
        let created = BotService::create_webhook(
            &svc,
            actor(),
            channel,
            bot,
            WebhookPreset::Plain,
            Some(a_format()),
            None,
        )
        .await
        .expect("created");
        assert_eq!(created.webhook.template, Some(a_format()));

        // Neither refusal reaches the repository: nothing above expects an insert.
        for (preset, template) in [
            (WebhookPreset::Github, a_format()),
            (
                WebhookPreset::Plain,
                serde_json::json!({ "version": 1, "rules": [{ "post": "{" }] }),
            ),
        ] {
            let fx = Fx::new();
            let (channel, bot) = (fx.channel, fx.bot);
            let fx = fx
                .with_role(Some(ServerMemberRole::Moderator))
                .with_conversation(|_| {})
                .with_bot()
                .with_channel_members(|_| true);
            let (svc, _) = fx.build();
            assert_validation(
                BotService::create_webhook(
                    &svc,
                    actor(),
                    channel,
                    bot,
                    preset,
                    Some(template),
                    None,
                )
                .await,
            );
        }
    }

    /// A fixture whose webhook `webhook` is found and whose channel `shape` says.
    fn updating(
        role: Option<ServerMemberRole>,
        webhook: impl Fn(&Fx) -> IncomingWebhook,
        shape: impl Fn(&mut Conversation) + Send + 'static,
    ) -> (Fx, IncomingWebhookId) {
        let mut fx = Fx::new().with_role(role).with_conversation(shape);
        let found = webhook(&fx);
        let id = found.id;
        fx.bots
            .expect_find_webhook()
            .returning(move |_| ok(Some(found.clone())));
        (fx, id)
    }

    async fn update(
        svc: &DefaultBotService,
        id: IncomingWebhookId,
        preset: WebhookPreset,
        template: Option<serde_json::Value>,
    ) -> ServiceResult<IncomingWebhookDto> {
        BotService::update_webhook_format(svc, actor(), id, preset, template).await
    }

    #[tokio::test]
    async fn an_update_stores_the_format_and_audits_ids_alone() {
        let (mut fx, id) = updating(Some(ServerMemberRole::Moderator), a_webhook, |_| {});
        let (server_id, channel, bot) = (fx.server_id, fx.channel, fx.bot);
        fx.bots
            .expect_update_webhook_format()
            .withf(|_, preset, template| {
                *preset == WebhookPreset::Plain && template.as_ref() == Some(&a_format())
            })
            .times(1)
            .returning(move |id, preset, template| {
                ok(Some(IncomingWebhook {
                    id,
                    server_id,
                    conversation_id: channel,
                    bot_user_id: bot,
                    preset,
                    template,
                    created_at: Utc::now(),
                    last_used_at: None,
                    revoked_at: None,
                }))
            });
        let (svc, audit) = fx.build();
        let updated = update(&svc, id, WebhookPreset::Plain, Some(a_format()))
            .await
            .expect("updated");
        assert_eq!(updated.template, Some(a_format()));
        let entries = audit.0.lock().expect("lock");
        let [entry] = entries.as_slice() else {
            panic!("{entries:?}");
        };
        assert_eq!(entry.command_id, AUDIT_WEBHOOK_UPDATE);
        assert_eq!(
            entry.args_redacted,
            serde_json::json!({
                "webhook_id": id.to_string(),
                "conversation_id": channel.to_string(),
                "bot_user_id": bot.to_string(),
            })
        );
    }

    #[tokio::test]
    async fn an_update_back_to_a_preset_stores_no_format() {
        let (mut fx, id) = updating(Some(ServerMemberRole::Owner), a_webhook, |_| {});
        let (server_id, channel, bot) = (fx.server_id, fx.channel, fx.bot);
        fx.bots
            .expect_update_webhook_format()
            .withf(|_, preset, template| *preset == WebhookPreset::Github && template.is_none())
            .times(1)
            .returning(move |id, preset, template| {
                ok(Some(IncomingWebhook {
                    id,
                    server_id,
                    conversation_id: channel,
                    bot_user_id: bot,
                    preset,
                    template,
                    created_at: Utc::now(),
                    last_used_at: None,
                    revoked_at: None,
                }))
            });
        let (svc, _) = fx.build();
        let updated = update(&svc, id, WebhookPreset::Github, None)
            .await
            .expect("updated");
        assert_eq!(updated.preset, WebhookPreset::Github);
    }

    #[tokio::test]
    async fn an_update_answers_not_found_outside_the_actors_moderation() {
        for role in [None, Some(ServerMemberRole::Member)] {
            let (fx, id) = updating(role, a_webhook, |_| {});
            let (svc, _) = fx.build();
            assert!(matches!(
                update(&svc, id, WebhookPreset::Plain, Some(a_format())).await,
                Err(ServiceError::NotFound)
            ));
        }
        let mut fx = Fx::new();
        fx.bots.expect_find_webhook().returning(|_| ok(None));
        let (svc, _) = fx.build();
        assert!(matches!(
            update(&svc, IncomingWebhookId::new(), WebhookPreset::Plain, None).await,
            Err(ServiceError::NotFound)
        ));
    }

    #[tokio::test]
    async fn an_update_of_a_private_channel_takes_a_moderator_who_is_in_it() {
        let (fx, id) = updating(
            Some(ServerMemberRole::Moderator),
            a_webhook,
            |conversation| conversation.is_private = true,
        );
        let fx = fx.with_channel_members(|_| false);
        let (svc, _) = fx.build();
        assert!(matches!(
            update(&svc, id, WebhookPreset::Plain, Some(a_format())).await,
            Err(ServiceError::PermissionDenied)
        ));
    }

    #[tokio::test]
    async fn an_update_refuses_a_revoked_webhook_and_a_format_that_does_not_pass() {
        let revoked = |fx: &Fx| IncomingWebhook {
            revoked_at: Some(Utc::now()),
            ..a_webhook(fx)
        };
        let (fx, id) = updating(Some(ServerMemberRole::Moderator), revoked, |_| {});
        let (svc, _) = fx.build();
        assert_validation(update(&svc, id, WebhookPreset::Plain, Some(a_format())).await);

        for (preset, template) in [
            (WebhookPreset::Github, a_format()),
            (
                WebhookPreset::Plain,
                serde_json::json!({ "version": 1, "rules": [{ "post": "x", "when": [{ "header": "X-Api-Key", "equals": "k" }] }] }),
            ),
        ] {
            let (fx, id) = updating(Some(ServerMemberRole::Moderator), a_webhook, |_| {});
            let (svc, _) = fx.build();
            let refused = update(&svc, id, preset, Some(template)).await;
            assert_validation(refused);
        }
    }

    #[tokio::test]
    async fn an_update_that_loses_a_race_with_a_revoke_is_refused_and_not_audited() {
        let (mut fx, id) = updating(Some(ServerMemberRole::Moderator), a_webhook, |_| {});
        fx.bots
            .expect_update_webhook_format()
            .returning(|_, _, _| ok(None));
        let (svc, audit) = fx.build();
        assert_validation(update(&svc, id, WebhookPreset::Plain, None).await);
        assert!(audit.0.lock().expect("lock").is_empty());
    }

    async fn preview(
        svc: &DefaultBotService,
        server_id: ServerId,
        template: serde_json::Value,
        headers: Vec<(String, String)>,
        body: &str,
    ) -> ServiceResult<WebhookPreviewDto> {
        BotService::preview_webhook_format(
            svc,
            actor(),
            server_id,
            template,
            headers,
            body.to_string(),
        )
        .await
    }

    #[tokio::test]
    async fn a_preview_says_what_would_post_or_why_not() {
        let fx = Fx::new().with_role(Some(ServerMemberRole::Moderator));
        let server_id = fx.server_id;
        let (svc, audit) = fx.build();
        for (template, headers, body, expected) in [
            (
                a_format(),
                vec![],
                r#"{"status":"failed","host":"db-1"}"#,
                WebhookPreviewDto::Posts {
                    rule: 1,
                    text: "Backup failed on db-1".to_string(),
                    link: None,
                },
            ),
            (
                a_format(),
                vec![],
                r#"{"status":"ok"}"#,
                WebhookPreviewDto::NoRule,
            ),
            (
                a_format(),
                vec![],
                r#"{"status":"failed"}"#,
                WebhookPreviewDto::NoValue {
                    rule: 1,
                    placeholder: "host".to_string(),
                },
            ),
            (
                a_format(),
                vec![],
                "status=failed",
                WebhookPreviewDto::BodyRefused {
                    reason: "The body must be JSON.".to_string(),
                },
            ),
            (
                serde_json::json!({ "version": 1, "rules": [{ "post": "x" }, { "post": "{" }] }),
                vec![],
                "{}",
                WebhookPreviewDto::FormatRefused {
                    rule: Some(2),
                    reason: "A `{` has no `}`. Write `{{` for a brace.".to_string(),
                },
            ),
            (
                serde_json::json!({ "version": 1, "rules": [
                    { "when": [{ "header": "X-Event", "equals": "push" }], "post": "pushed" },
                ]}),
                vec![("X-Event".to_string(), "push".to_string())],
                "not json",
                WebhookPreviewDto::Posts {
                    rule: 1,
                    text: "pushed".to_string(),
                    link: None,
                },
            ),
            (
                serde_json::json!({ "version": 1, "rules": [
                    { "when": [{ "header": "X-Event", "equals": "e".repeat(65) }], "post": "x" },
                ]}),
                vec![("X-Event".to_string(), "e".repeat(65))],
                "{}",
                WebhookPreviewDto::NoRule,
            ),
        ] {
            assert_eq!(
                preview(&svc, server_id, template, headers, body)
                    .await
                    .expect("a preview"),
                expected
            );
        }
        assert!(
            audit.0.lock().expect("lock").is_empty(),
            "nothing is recorded"
        );
    }

    #[tokio::test]
    async fn a_preview_is_for_a_moderator_and_a_sample_of_at_most_256_kib() {
        for role in [None, Some(ServerMemberRole::Member)] {
            let fx = Fx::new().with_role(role);
            let server_id = fx.server_id;
            let (svc, _) = fx.build();
            assert!(matches!(
                preview(&svc, server_id, a_format(), vec![], "{}").await,
                Err(ServiceError::PermissionDenied)
            ));
        }
        let fx = Fx::new().with_role(Some(ServerMemberRole::Owner));
        let server_id = fx.server_id;
        let (svc, _) = fx.build();
        let at_limit = " ".repeat(MAX_SAMPLE_BYTES - 7);
        assert!(
            preview(
                &svc,
                server_id,
                a_format(),
                vec![("x-a".to_string(), "bc".to_string())],
                &format!("{{}}{at_limit}")
            )
            .await
            .is_ok()
        );
        assert_validation(
            preview(
                &svc,
                server_id,
                a_format(),
                vec![("x-a".to_string(), "bcd".to_string())],
                &format!("{{}}{at_limit}"),
            )
            .await,
        );
    }

    #[tokio::test]
    async fn the_new_calls_refuse_while_the_feature_is_off() {
        let mut fx = Fx::new();
        fx.disabled.push(ServerFeature::Bots);
        let server_id = fx.server_id;
        let (svc, _) = fx.build();
        let off =
            |e: &ServiceError| matches!(e, ServiceError::FeatureDisabled(ServerFeature::Bots));
        assert!(off(&update(
            &svc,
            IncomingWebhookId::new(),
            WebhookPreset::Plain,
            None
        )
        .await
        .expect_err("off")));
        assert!(off(&BotService::webhook_formats(&svc).expect_err("off")));
        assert!(off(&preview(&svc, server_id, a_format(), vec![], "{}")
            .await
            .expect_err("off")));
    }

    #[test]
    fn the_shipped_formats_are_listed_with_their_samples() {
        let formats = shipped_formats().expect("listed");
        let keys: Vec<&str> = formats.iter().map(|format| format.key.as_str()).collect();
        assert_eq!(keys, ["github", "grafana"]);
        for format in &formats {
            assert!(parse_format(&format.format).is_ok(), "{}", format.key);
            assert!(!format.setup_note.is_empty());
        }
        assert_eq!(
            formats[0].sample_headers,
            [("X-GitHub-Event".to_string(), "pull_request".to_string())]
        );
    }

    #[test]
    fn a_stored_format_is_used_and_one_that_does_not_pass_is_internal() {
        let webhook = |template: serde_json::Value| IncomingWebhookDto {
            id: IncomingWebhookId::new(),
            server_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            bot_user_id: UserId::new(),
            preset: WebhookPreset::Plain,
            template: Some(template),
            created_at: Utc::now(),
            last_used_at: None,
            revoked_at: None,
        };
        let body = br#"{"status":"failed","host":"db-1"}"#;
        assert_eq!(
            super::text_of(&webhook(a_format()), &[], body).expect("text"),
            Some("Backup failed on db-1".to_string())
        );
        let unreadable = serde_json::json!({ "version": 2, "rules": [] });
        assert!(matches!(
            super::text_of(&webhook(unreadable), &[], body),
            Err(ServiceError::Internal(_))
        ));
    }

    // ── test sessions ────────────────────────────────────────────────────

    /// A service of `fx` holding its sessions in a store the test can look into.
    fn with_store(
        fx: Fx,
    ) -> (
        DefaultBotService,
        Arc<RecordingAudit>,
        Arc<InMemoryTestSessionStore>,
    ) {
        let store = Arc::new(InMemoryTestSessionStore::new());
        let (svc, audit) = fx.build();
        let shared: Arc<dyn WebhookTestSessionStore> = store.clone();
        (svc.with_test_sessions(shared), audit, store)
    }

    async fn start(
        svc: &DefaultBotService,
        who: Actor,
        server_id: ServerId,
        webhook_id: Option<IncomingWebhookId>,
    ) -> ServiceResult<rim_services::bot::dto::StartedTestSessionDto> {
        BotService::start_test_session(svc, who, server_id, webhook_id).await
    }

    #[tokio::test]
    async fn an_unbound_session_answers_its_token_once_and_holds_only_its_hash() {
        let fx = Fx::new().with_role(Some(ServerMemberRole::Moderator));
        let server_id = fx.server_id;
        let (svc, audit, store) = with_store(fx);
        let started = start(&svc, actor(), server_id, None)
            .await
            .expect("started");
        let token = started.token.clone().expect("a token");
        assert!(is_webhook_token_shape(&token));
        let held = format!("{store:?}");
        assert!(!held.contains(&token), "the store shows the token");
        assert!(
            !held.contains(&hash_token(&token)),
            "the store shows the hash"
        );
        assert!(
            store
                .unbound_by_hash(&hash_token(&token), Utc::now())
                .is_some(),
            "the store finds the session by the hash"
        );
        assert!(store.unbound_by_hash(&token, Utc::now()).is_none());
        assert!(!format!("{started:?}").contains(&token));
        assert_eq!(
            BotService::resolve_test_session(&svc, &token),
            Some(started.id)
        );
        assert_eq!(BotService::resolve_test_session(&svc, "rimhook_nope"), None);
        let entries = audit.0.lock().expect("lock");
        let [entry] = entries.as_slice() else {
            panic!("{entries:?}");
        };
        assert_eq!(
            entry.command_id,
            super::super::test_session::AUDIT_TEST_SESSION_START
        );
        assert_eq!(
            entry.args_redacted,
            serde_json::json!({ "session_id": started.id.to_string() })
        );
    }

    #[tokio::test]
    async fn a_session_takes_a_moderator_and_a_workspace_holds_three() {
        for role in [None, Some(ServerMemberRole::Member)] {
            let fx = Fx::new().with_role(role);
            let server_id = fx.server_id;
            let (svc, _, _) = with_store(fx);
            assert!(matches!(
                start(&svc, actor(), server_id, None).await,
                Err(ServiceError::PermissionDenied)
            ));
        }
        let fx = Fx::new().with_role(Some(ServerMemberRole::Owner));
        let server_id = fx.server_id;
        let (svc, _, _) = with_store(fx);
        for _ in 0..MAX_TEST_SESSIONS_PER_WORKSPACE {
            start(&svc, actor(), server_id, None).await.expect("room");
        }
        assert!(matches!(
            start(&svc, actor(), server_id, None).await,
            Err(ServiceError::RateLimited { .. })
        ));
    }

    #[tokio::test]
    async fn a_bound_session_takes_the_standing_of_changing_that_webhook() {
        // A webhook of another workspace is not found.
        let (fx, id) = updating(Some(ServerMemberRole::Moderator), a_webhook, |_| {});
        let (svc, _, _) = with_store(fx);
        assert!(matches!(
            start(&svc, actor(), ServerId::new(), Some(id)).await,
            Err(ServiceError::NotFound)
        ));
        // A private channel the moderator is not in.
        let (fx, id) = updating(
            Some(ServerMemberRole::Moderator),
            a_webhook,
            |conversation| conversation.is_private = true,
        );
        let server_id = fx.server_id;
        let fx = fx.with_channel_members(|_| false);
        let (svc, _, _) = with_store(fx);
        assert!(matches!(
            start(&svc, actor(), server_id, Some(id)).await,
            Err(ServiceError::PermissionDenied)
        ));
        // A revoked webhook.
        let revoked = |fx: &Fx| IncomingWebhook {
            revoked_at: Some(Utc::now()),
            ..a_webhook(fx)
        };
        let (fx, id) = updating(Some(ServerMemberRole::Moderator), revoked, |_| {});
        let server_id = fx.server_id;
        let (svc, _, _) = with_store(fx);
        assert_validation(start(&svc, actor(), server_id, Some(id)).await);
        // One that passes has no token of its own.
        let (fx, id) = updating(Some(ServerMemberRole::Moderator), a_webhook, |_| {});
        let server_id = fx.server_id;
        let (svc, _, _) = with_store(fx);
        let started = start(&svc, actor(), server_id, Some(id))
            .await
            .expect("started");
        assert_eq!(started.token, None);
    }

    #[tokio::test]
    async fn a_held_delivery_keeps_no_refused_header_and_its_body_as_it_came() {
        let fx = Fx::new().with_role(Some(ServerMemberRole::Moderator));
        let server_id = fx.server_id;
        let (svc, _, store) = with_store(fx);
        let me = actor();
        let id = start(&svc, me, server_id, None).await.expect("started").id;
        let headers: Vec<(String, String)> = [
            ("x-github-event", "push"),
            ("x-gitlab-token", "secret-gitlab"),
            ("x-hub-signature-256", "sha256=secret-hub"),
            ("x-api-key", "secret-key"),
            ("x-authorization", "secret-auth"),
            ("x-cookie", "secret-cookie"),
            ("x-webhook-secret", "secret-secret"),
        ]
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
        let body = "line\u{0}\u{1b}[31m\u{202e}end";
        BotService::record_test_delivery(
            &svc,
            id,
            Some("application/json".to_string()),
            &headers,
            body.as_bytes(),
            false,
        );
        BotService::record_test_delivery(&svc, id, None, &[], &[0xff, 0xfe], false);
        BotService::record_test_delivery(
            &svc,
            id,
            None,
            &[],
            &vec![b'a'; MAX_TEST_BODY_BYTES + 1],
            false,
        );
        BotService::record_test_delivery(&svc, id, None, &[], &[], true);
        let read = BotService::test_deliveries(&svc, me, id, 0)
            .await
            .expect("read");
        let [kept, not_text, large, unread] = read.deliveries.as_slice() else {
            panic!("{:?}", read.deliveries);
        };
        assert_eq!(
            kept.headers,
            [("x-github-event".to_string(), "push".to_string())]
        );
        assert_eq!(kept.content_type.as_deref(), Some("application/json"));
        assert_eq!(kept.body, TestBody::Text(body.to_string()));
        assert_eq!(not_text.body, TestBody::NotText);
        assert_eq!(large.body, TestBody::TooLarge);
        assert_eq!(unread.body, TestBody::TooLarge);
        assert!(
            !format!("{store:?}").contains("secret-"),
            "a refused header is held"
        );
    }

    #[tokio::test]
    async fn only_the_starter_reads_or_ends_a_session_and_not_once_they_lose_the_standing() {
        let role = Arc::new(Mutex::new(Some(ServerMemberRole::Moderator)));
        let mut fx = Fx::new();
        let current = Arc::clone(&role);
        fx.members
            .expect_get_member()
            .returning(move |server_id, user_id| {
                Ok(
                    (*current.lock().expect("lock")).map(|member_role| ServerMember {
                        id: ServerMemberId::new(),
                        server_id,
                        user_id,
                        member_role,
                        joined_at: Utc::now(),
                    }),
                )
            });
        let server_id = fx.server_id;
        let (svc, audit, _) = with_store(fx);
        let me = actor();
        let id = start(&svc, me, server_id, None).await.expect("started").id;
        assert!(BotService::test_deliveries(&svc, me, id, 0).await.is_ok());

        // Another moderator of the same workspace.
        let other = actor();
        assert!(matches!(
            BotService::test_deliveries(&svc, other, id, 0).await,
            Err(ServiceError::NotFound)
        ));
        assert!(matches!(
            BotService::end_test_session(&svc, other, id).await,
            Err(ServiceError::NotFound)
        ));

        // The starter, no longer a moderator.
        *role.lock().expect("lock") = Some(ServerMemberRole::Member);
        assert!(matches!(
            BotService::test_deliveries(&svc, me, id, 0).await,
            Err(ServiceError::NotFound)
        ));
        *role.lock().expect("lock") = Some(ServerMemberRole::Moderator);

        BotService::end_test_session(&svc, me, id)
            .await
            .expect("ended");
        assert!(matches!(
            BotService::test_deliveries(&svc, me, id, 0).await,
            Err(ServiceError::NotFound)
        ));
        let commands: Vec<String> = audit
            .0
            .lock()
            .expect("lock")
            .iter()
            .map(|entry| entry.command_id.clone())
            .collect();
        assert_eq!(
            commands,
            [
                super::super::test_session::AUDIT_TEST_SESSION_START,
                super::super::test_session::AUDIT_TEST_SESSION_END
            ]
        );
    }

    #[tokio::test]
    async fn a_bound_session_gets_a_copy_and_ends_when_its_webhook_is_revoked() {
        let (mut fx, id) = updating(Some(ServerMemberRole::Moderator), a_webhook, |_| {});
        let server_id = fx.server_id;
        fx.bots.expect_revoke_webhook().returning(move |id, at| {
            ok(Some(IncomingWebhook {
                id,
                server_id,
                conversation_id: ConversationId::new(),
                bot_user_id: UserId::new(),
                preset: WebhookPreset::Plain,
                template: None,
                created_at: at,
                last_used_at: None,
                revoked_at: Some(at),
            }))
        });
        let (svc, _, store) = with_store(fx);
        let me = actor();
        let session = start(&svc, me, server_id, Some(id))
            .await
            .expect("started")
            .id;
        BotService::copy_to_test_sessions(&svc, id, None, &[], b"{}", false);
        BotService::copy_to_test_sessions(&svc, IncomingWebhookId::new(), None, &[], b"no", false);
        let read = BotService::test_deliveries(&svc, me, session, 0)
            .await
            .expect("read");
        assert_eq!(read.deliveries.len(), 1);
        assert_eq!(read.deliveries[0].body, TestBody::Text("{}".to_string()));
        BotService::revoke_webhook(&svc, me, id)
            .await
            .expect("revoked");
        assert!(store.get(session, Utc::now()).is_none());
    }

    #[tokio::test]
    async fn the_sweep_drops_a_session_past_its_expiry() {
        let fx = Fx::new();
        let (svc, _, store) = with_store(fx);
        let long_ago = Utc::now() - chrono::Duration::hours(2);
        let old = store
            .open(
                NewTestSession {
                    server_id: ServerId::new(),
                    owner: UserId::new(),
                    webhook_id: None,
                    token_hash: Some("old".to_string()),
                },
                long_ago,
            )
            .expect("open");
        assert!(store.get(old.id, long_ago).is_some());
        BotService::sweep_test_sessions(&svc);
        assert!(store.get(old.id, long_ago).is_none());
    }

    /// A create fixture whose insert records the token hash it was given.
    fn creating(fx: Fx) -> (Fx, Arc<Mutex<Option<String>>>) {
        let stored = Arc::new(Mutex::new(None));
        let seen = Arc::clone(&stored);
        let mut fx = fx
            .with_role(Some(ServerMemberRole::Moderator))
            .with_conversation(|_| {})
            .with_bot()
            .with_channel_members(|_| true)
            .with_no_active_webhook();
        fx.bots.expect_lock_bot().returning(|_| ok(true));
        fx.bots.expect_create_webhook().returning(move |new| {
            *seen.lock().expect("lock") = Some(new.token_hash.clone());
            ok(IncomingWebhook {
                id: IncomingWebhookId::new(),
                server_id: new.server_id,
                conversation_id: new.conversation_id,
                bot_user_id: new.bot_user_id,
                preset: new.preset,
                template: new.template,
                created_at: Utc::now(),
                last_used_at: None,
                revoked_at: None,
            })
        });
        (fx, stored)
    }

    async fn create_keeping(
        svc: &DefaultBotService,
        who: Actor,
        channel: ConversationId,
        bot: UserId,
        session: Option<WebhookTestSessionId>,
    ) -> ServiceResult<CreatedIncomingWebhookDto> {
        BotService::create_webhook(svc, who, channel, bot, WebhookPreset::Plain, None, session)
            .await
    }

    #[tokio::test]
    async fn saving_keeps_the_callers_open_session_url_and_answers_no_token() {
        let (fx, stored) = creating(Fx::new());
        let (server_id, channel, bot) = (fx.server_id, fx.channel, fx.bot);
        let (svc, audit, store) = with_store(fx);
        let me = actor();
        let started = start(&svc, me, server_id, None).await.expect("started");
        let token = started.token.expect("token");
        let created = create_keeping(&svc, me, channel, bot, Some(started.id))
            .await
            .expect("created");
        assert!(created.kept_test_url);
        assert_eq!(created.token, "");
        assert_eq!(
            stored.lock().expect("lock").as_deref(),
            Some(hash_token(&token).as_str())
        );
        assert!(
            store.get(started.id, Utc::now()).is_none(),
            "promoting ends it"
        );
        let commands: Vec<String> = audit
            .0
            .lock()
            .expect("lock")
            .iter()
            .map(|entry| entry.command_id.clone())
            .collect();
        assert!(
            commands.iter().any(|c| c == AUDIT_WEBHOOK_CREATE),
            "{commands:?}"
        );
        assert_eq!(
            commands.last().map(String::as_str),
            Some(super::super::test_session::AUDIT_TEST_SESSION_END)
        );

        // Used again, it is gone: a new token, not kept.
        let (fx, _) = creating(Fx::new());
        let (channel, bot) = (fx.channel, fx.bot);
        let (svc2, _, _) = with_store(fx);
        let again = create_keeping(&svc2, me, channel, bot, Some(started.id))
            .await
            .expect("created");
        assert!(!again.kept_test_url);
        assert!(is_webhook_token_shape(&again.token));
        let again = create_keeping(&svc, me, channel, bot, Some(started.id))
            .await
            .expect("created");
        assert!(!again.kept_test_url, "a session is promoted once");
        assert!(is_webhook_token_shape(&again.token));
    }

    #[tokio::test]
    async fn saving_with_an_expired_session_gives_a_new_token() {
        let (fx, stored) = creating(Fx::new());
        let (server_id, channel, bot) = (fx.server_id, fx.channel, fx.bot);
        let (svc, _, store) = with_store(fx);
        let me = actor();
        let expired = store
            .open(
                NewTestSession {
                    server_id,
                    owner: me.user_id,
                    webhook_id: None,
                    token_hash: Some("expired-hash".to_string()),
                },
                Utc::now() - chrono::Duration::hours(2),
            )
            .expect("open");
        let created = create_keeping(&svc, me, channel, bot, Some(expired.id))
            .await
            .expect("created");
        assert!(!created.kept_test_url);
        assert_eq!(
            stored.lock().expect("lock").as_deref(),
            Some(hash_token(&created.token).as_str())
        );
    }

    #[tokio::test]
    async fn saving_with_someone_elses_another_workspaces_or_a_bound_session_is_refused() {
        let (fx, _) = creating(Fx::new());
        let (server_id, channel, bot) = (fx.server_id, fx.channel, fx.bot);
        let (svc, _, store) = with_store(fx);
        let me = actor();
        let theirs = start(&svc, actor(), server_id, None)
            .await
            .expect("started")
            .id;
        let elsewhere = store
            .open(
                NewTestSession {
                    server_id: ServerId::new(),
                    owner: me.user_id,
                    webhook_id: None,
                    token_hash: Some("elsewhere".to_string()),
                },
                Utc::now(),
            )
            .expect("open")
            .id;
        let bound = store
            .open(
                NewTestSession {
                    server_id,
                    owner: me.user_id,
                    webhook_id: Some(IncomingWebhookId::new()),
                    token_hash: None,
                },
                Utc::now(),
            )
            .expect("open")
            .id;
        for session in [theirs, elsewhere, bound] {
            assert_validation(create_keeping(&svc, me, channel, bot, Some(session)).await);
            assert!(
                store.get(session, Utc::now()).is_some(),
                "a refusal ends nothing"
            );
        }
    }

    #[tokio::test]
    async fn sessions_refuse_while_the_feature_is_off() {
        let mut fx = Fx::new();
        fx.disabled.push(ServerFeature::Bots);
        let server_id = fx.server_id;
        let (svc, _, store) = with_store(fx);
        let off =
            |e: &ServiceError| matches!(e, ServiceError::FeatureDisabled(ServerFeature::Bots));
        assert!(off(&start(&svc, actor(), server_id, None)
            .await
            .expect_err("off")));
        let open = store
            .open(
                NewTestSession {
                    server_id,
                    owner: UserId::new(),
                    webhook_id: None,
                    token_hash: Some(hash_token(GOOD_TOKEN)),
                },
                Utc::now(),
            )
            .expect("open");
        assert_eq!(BotService::resolve_test_session(&svc, GOOD_TOKEN), None);
        assert!(off(&BotService::test_deliveries(&svc, actor(), open.id, 0)
            .await
            .expect_err("off")));
        assert!(off(&BotService::end_test_session(&svc, actor(), open.id)
            .await
            .expect_err("off")));
    }

    // ── text ─────────────────────────────────────────────────────────────

    /// The text a delivery to a webhook of `preset` with no format of its own becomes, sent with
    /// `event` as its `X-GitHub-Event` header when there is one.
    fn text_of(
        preset: WebhookPreset,
        event: Option<&str>,
        body: &[u8],
    ) -> ServiceResult<Option<String>> {
        let webhook = IncomingWebhookDto {
            id: IncomingWebhookId::new(),
            server_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            bot_user_id: UserId::new(),
            preset,
            template: None,
            created_at: Utc::now(),
            last_used_at: None,
            revoked_at: None,
        };
        let headers: Vec<(String, String)> = event
            .map(|event| ("x-github-event".to_string(), event.to_string()))
            .into_iter()
            .collect();
        super::text_of(&webhook, &headers, body)
    }

    #[test]
    fn each_preset_reads_its_body_and_a_github_body_needs_an_event() {
        let body = br#"{"text":"hello"}"#;
        assert_eq!(
            text_of(WebhookPreset::Plain, Some("issues"), body).expect("text"),
            Some("hello".to_string())
        );
        assert_eq!(
            text_of(WebhookPreset::Github, None, body).expect("text"),
            None
        );
        assert_eq!(
            text_of(WebhookPreset::Github, Some("ping"), body).expect("text"),
            None
        );
        assert!(matches!(
            text_of(WebhookPreset::Plain, None, b"nope"),
            Err(ServiceError::Validation(_))
        ));
        assert!(matches!(
            text_of(WebhookPreset::Github, Some("issues"), b"nope"),
            Err(ServiceError::Validation(_))
        ));
    }
}
