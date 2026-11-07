//! Incoming webhooks: who may create and revoke one, the channel it is bound to, whether a
//! presented token works and what its body becomes. Running the post is the transport's, through
//! the command the bot API runs.

use chrono::Utc;
use tracing::{instrument, warn};

use rim_auth::session_token::{generate_webhook_token, hash_token, is_webhook_token_shape};
use rim_core::bot::model::{IncomingWebhookId, NewIncomingWebhook, WebhookPreset};
use rim_core::conversation::model::{ConversationId, ConversationType};
use rim_core::repository::error::RepoErrorKind;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_services::bot::dto::{CreatedIncomingWebhookDto, IncomingWebhookDto};
use rim_services::command::verb::TypedVerb;
use rim_services::conversation::command::AddConversationMembersCmd;
use rim_services::{Actor, ServiceError, ServiceResult};

use super::webhook_text::{DecodeError, github_text, plain_text};
use super::{DefaultBotService, last_use_is_stale};

const AUDIT_WEBHOOK_CREATE: &str = "bot.webhook.create";
const AUDIT_WEBHOOK_REVOKE: &str = "bot.webhook.revoke";

/// The refusal of a second webhook on a channel.
const CHANNEL_HAS_ONE: &str = "This channel already has a webhook. Revoke it first.";

/// The refusal of a webhook for a bot that is deactivated.
const DEACTIVATED_BOT: &str = "A deactivated bot cannot have a webhook.";

/// The text `body` becomes under `preset`, or `None` when there is nothing to post. The GitHub
/// preset posts nothing without an event to go by.
///
/// # Errors
/// `Validation` for a body the preset cannot read.
pub(super) fn text_of(
    preset: WebhookPreset,
    event: Option<&str>,
    body: &[u8],
) -> ServiceResult<Option<String>> {
    let decoded = match (preset, event) {
        (WebhookPreset::Plain, _) => plain_text(body),
        (WebhookPreset::Github, Some(event)) => github_text(event, body),
        (WebhookPreset::Github, None) => Ok(None),
    };
    decoded.map_err(|DecodeError(reason)| ServiceError::Validation(reason.to_string()))
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

        let token = generate_webhook_token();
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
        let created = repos
            .bot_repo
            .create_webhook(NewIncomingWebhook {
                server_id,
                conversation_id,
                bot_user_id,
                preset,
                token_hash: hash_token(&token),
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
        Ok(CreatedIncomingWebhookDto {
            webhook: created.into(),
            token,
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
            sees_member_joins: false,
        }
    }

    fn a_webhook(fx: &Fx) -> IncomingWebhook {
        IncomingWebhook {
            id: IncomingWebhookId::new(),
            server_id: fx.server_id,
            conversation_id: fx.channel,
            bot_user_id: fx.bot,
            preset: WebhookPreset::Plain,
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
        BotService::create_webhook(svc, actor(), channel, bot, WebhookPreset::Github).await
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

    // ── text ─────────────────────────────────────────────────────────────

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
