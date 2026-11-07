use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::bot::model::{
    BotFeedPosition, BotTokenId, IncomingWebhookId, WebhookPreset, WebhookTestSessionId,
};
use rim_core::conversation::model::ConversationId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use crate::bot::dto::{
    BotActorDto, BotDto, BotEventsPage, BotInfoDto, BotMentionCursor, BotMentionsPage, BotTokenDto,
    CreatedBotDto, CreatedBotTokenDto, CreatedIncomingWebhookDto, IncomingWebhookDto,
    StartedTestSessionDto, TestDeliveriesDto, WebhookFormatInfoDto, WebhookPreviewDto,
};
use crate::{Actor, ServiceResult};

/// Bot accounts of a workspace, their tokens and the incoming webhooks bound to them.
///
/// Errors common to the methods with an actor: `FeatureDisabled` while the instance has the `bots`
/// feature off, `PermissionDenied` when the actor lacks the standing, `NotFound` for an unknown
/// bot, `Validation` for bad input.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait BotService: Send + Sync {
    /// Create a bot, a `GUEST` of the workspace in no channel, and its first token. Owner or
    /// Moderator.
    ///
    /// # Errors
    /// `Validation` for the direct space, a bad field or a workspace at its cap on active bots.
    async fn create_bot(
        &self,
        actor: Actor,
        server_id: ServerId,
        name: String,
        run_by: String,
        reads_go_to: String,
        sees_member_joins: bool,
    ) -> ServiceResult<CreatedBotDto>;

    /// Change the fields given. Owner or Moderator.
    async fn update_bot(
        &self,
        actor: Actor,
        user_id: UserId,
        name: Option<String>,
        run_by: Option<String>,
        reads_go_to: Option<String>,
        sees_member_joins: Option<bool>,
    ) -> ServiceResult<BotDto>;

    /// Deactivate a bot: its tokens stop working, its webhooks are revoked and it leaves the
    /// workspace and its channels. Its messages stay. Doing it again changes nothing. Owner or
    /// Moderator.
    async fn deactivate_bot(&self, actor: Actor, user_id: UserId) -> ServiceResult<BotDto>;

    /// Create a token for an active bot. Owner or Moderator.
    ///
    /// # Errors
    /// `Validation` for a bad label, a deactivated bot or a bot at its cap on active tokens.
    async fn create_token(
        &self,
        actor: Actor,
        user_id: UserId,
        label: String,
    ) -> ServiceResult<CreatedBotTokenDto>;

    /// Revoke a token. Revoking it again changes nothing. Owner or Moderator of its bot's
    /// workspace.
    ///
    /// # Errors
    /// `NotFound` for an unknown token and for a token of a bot in a workspace the actor does not
    /// moderate.
    async fn revoke_token(&self, actor: Actor, token_id: BotTokenId) -> ServiceResult<BotTokenDto>;

    /// The workspace's bots, deactivated ones included. Any member.
    async fn list_bots(&self, actor: Actor, server_id: ServerId) -> ServiceResult<Vec<BotDto>>;

    /// The bot's tokens, revoked ones included. Owner or Moderator.
    async fn list_tokens(&self, actor: Actor, user_id: UserId) -> ServiceResult<Vec<BotTokenDto>>;

    /// What everyone who can see a bot sees about it, for the bots among `user_ids`; other users
    /// are left out. Answers while the feature is off too: existing bots stay marked as bots.
    async fn bot_infos(&self, user_ids: &[UserId]) -> ServiceResult<Vec<BotInfoDto>>;

    /// Who `raw` acts as, or `None` when it is not a working token: wrong shape, unknown, revoked,
    /// of a deactivated, removed or banned bot, or while the feature is off. Every one of those
    /// looks the same to the caller.
    ///
    /// # Errors
    /// Propagates storage errors.
    async fn authenticate_token(&self, raw: &str) -> ServiceResult<Option<BotActorDto>>;

    /// The bot a working token acts as.
    ///
    /// # Errors
    /// `NotFound` when it is gone; propagates storage errors.
    async fn bot_of(&self, bot: BotActorDto) -> ServiceResult<BotDto>;

    /// Whether `conversation_id` is a plaintext channel of the bot's workspace with the bot a
    /// member of it. Every other case answers `false`, an unknown id among them.
    ///
    /// # Errors
    /// Propagates storage errors.
    async fn is_in_channel(
        &self,
        bot: BotActorDto,
        conversation_id: ConversationId,
    ) -> ServiceResult<bool>;

    /// The bot's mentions after `after`, or from the last 24 hours without it, at most `limit`.
    /// A deleted message and a channel the bot has left are left out. Recorded in the audit log,
    /// but for a poll that succeeded with nothing to hand over.
    ///
    /// # Errors
    /// Propagates storage errors.
    async fn list_mentions(
        &self,
        bot: BotActorDto,
        after: Option<BotMentionCursor>,
        limit: u32,
    ) -> ServiceResult<BotMentionsPage>;

    /// What happened after `after`, at most `limit` events, oldest first: messages in the bot's
    /// channels and, when it may see them, members joining. Without `after` the page is empty and
    /// its cursor is now, so nothing from before the first call is replayed. Rows younger than two
    /// seconds wait for a later call, so one committing late is not passed by the cursor. Recorded
    /// in the audit log with how many of each kind and never any text, but for a poll that
    /// succeeded with nothing to hand over.
    ///
    /// # Errors
    /// `NotFound` when the bot is gone; propagates storage errors.
    async fn list_events(
        &self,
        bot: BotActorDto,
        after: Option<BotFeedPosition>,
        limit: u32,
    ) -> ServiceResult<BotEventsPage>;

    /// Record in the audit log a read of the bot that ran without the command dispatcher, with the
    /// initiator `bot`. `error` is what it failed with. A failed write is logged, never returned.
    async fn record_read(
        &self,
        bot: BotActorDto,
        command_id: &'static str,
        args: serde_json::Value,
        error: Option<String>,
    );

    /// Create the channel's incoming webhook, bound to a bot of its workspace, and add the bot to
    /// the channel when it is not there. The token is shown this once. Owner or Moderator, and
    /// for a private channel one who is in it.
    ///
    /// # Errors
    /// `NotFound` for an unknown conversation or bot; `Validation` for a bot of another workspace
    /// or a deactivated one, and for anything but a plaintext channel that is not archived;
    /// `Conflict` when the channel already has a webhook that is not revoked. `template` is the
    /// webhook's own format, which needs the preset `Plain`; a format that is refused is
    /// `Validation`, naming the rule. With `test_session`, the caller's open unbound session of the
    /// workspace, the webhook keeps that session's token and the session ends; an expired or ended
    /// one gives a new token, and someone else's or another workspace's is `Validation`.
    async fn create_webhook(
        &self,
        actor: Actor,
        conversation_id: ConversationId,
        bot_user_id: UserId,
        preset: WebhookPreset,
        template: Option<serde_json::Value>,
        test_session: Option<WebhookTestSessionId>,
    ) -> ServiceResult<CreatedIncomingWebhookDto>;

    /// Open a test session held in memory for its starter alone: unbound with a URL that posts
    /// nothing, or bound to `webhook_id`, copying its deliveries. Owner or Moderator, and for a
    /// bound one the standing of changing that webhook's format.
    ///
    /// # Errors
    /// `PermissionDenied` without the standing; `NotFound` for an unknown webhook or one of another
    /// workspace; `Validation` for a revoked one; `RateLimited` when the workspace or the server
    /// holds as many sessions as it may.
    async fn start_test_session(
        &self,
        actor: Actor,
        server_id: ServerId,
        webhook_id: Option<IncomingWebhookId>,
    ) -> ServiceResult<StartedTestSessionDto>;

    /// End a test session at once. Its starter alone.
    ///
    /// # Errors
    /// `NotFound` for anyone else and for a session that ended.
    async fn end_test_session(&self, actor: Actor, id: WebhookTestSessionId) -> ServiceResult<()>;

    /// What a test session holds after `after`, oldest first, with the read extending it. Its
    /// starter alone, while they keep the standing to start it.
    ///
    /// # Errors
    /// `NotFound` for anyone else, for a session that ended and once the starter lacks the
    /// standing.
    async fn test_deliveries(
        &self,
        actor: Actor,
        id: WebhookTestSessionId,
        after: u64,
    ) -> ServiceResult<TestDeliveriesDto>;

    /// The open unbound test session `raw` is the token of, or `None`, also while bots are off.
    fn resolve_test_session(&self, raw: &str) -> Option<WebhookTestSessionId>;

    /// Hold a delivery in test session `id`: its `Content-Type`, the `x-` headers
    /// [`crate::bot::dto::delivery_headers`] kept, less those a format refuses, and its body. With
    /// `too_large` the body was too large to read, and `body` is ignored.
    fn record_test_delivery(
        &self,
        id: WebhookTestSessionId,
        content_type: Option<String>,
        headers: &[(String, String)],
        body: &[u8],
        too_large: bool,
    );

    /// Hold a delivery to `webhook_id` in each test session bound to it, as
    /// [`Self::record_test_delivery`] holds one.
    fn copy_to_test_sessions(
        &self,
        webhook_id: IncomingWebhookId,
        content_type: Option<String>,
        headers: &[(String, String)],
        body: &[u8],
        too_large: bool,
    );

    /// Drop every test session past its expiry.
    fn sweep_test_sessions(&self);

    /// Set how a webhook turns a delivery into a message: `preset` alone, or a `template` with the
    /// preset `Plain`. The URL stays the same. The standing of creating one.
    ///
    /// # Errors
    /// `NotFound` for an unknown webhook and for one of a workspace the actor does not moderate;
    /// `PermissionDenied` for a private channel the actor is not in; `Validation` for a revoked
    /// webhook and for a format that is refused, naming the rule.
    async fn update_webhook_format(
        &self,
        actor: Actor,
        id: IncomingWebhookId,
        preset: WebhookPreset,
        template: Option<serde_json::Value>,
    ) -> ServiceResult<IncomingWebhookDto>;

    /// The ready-made formats RIM ships.
    ///
    /// # Errors
    /// `FeatureDisabled` while bots are off.
    fn webhook_formats(&self) -> ServiceResult<Vec<WebhookFormatInfoDto>>;

    /// What a sample of `headers` and `body` would come to under `template`, as a delivery runs
    /// it. Nothing is posted or stored. Owner or Moderator of the workspace.
    ///
    /// # Errors
    /// `PermissionDenied` for anyone else; `Validation` for a sample over 256 KiB.
    async fn preview_webhook_format(
        &self,
        actor: Actor,
        server_id: ServerId,
        template: serde_json::Value,
        headers: Vec<(String, String)>,
        body: String,
    ) -> ServiceResult<WebhookPreviewDto>;

    /// Revoke a webhook. Revoking it again changes nothing. Owner or Moderator of its workspace.
    ///
    /// # Errors
    /// `NotFound` for an unknown webhook and for one of a workspace the actor does not moderate.
    async fn revoke_webhook(
        &self,
        actor: Actor,
        id: IncomingWebhookId,
    ) -> ServiceResult<IncomingWebhookDto>;

    /// The workspace's webhooks, revoked ones included. Owner or Moderator.
    async fn list_webhooks(
        &self,
        actor: Actor,
        server_id: ServerId,
    ) -> ServiceResult<Vec<IncomingWebhookDto>>;

    /// The webhook `raw` belongs to, with its use recorded, or `None` when it is not a working
    /// token: wrong shape, unknown, revoked, of a deactivated bot or one no longer in the channel,
    /// or while the feature is off. Every one of those looks the same to the caller.
    ///
    /// # Errors
    /// Propagates storage errors.
    async fn resolve_webhook(&self, raw: &str) -> ServiceResult<Option<IncomingWebhookDto>>;

    /// The text a delivery to `webhook` becomes, with no mention left in it, or `None` when there
    /// is nothing to post. `headers` are what [`crate::bot::dto::delivery_headers`] keeps.
    ///
    /// # Errors
    /// `Validation` for a body the webhook cannot read; `Internal` for a stored format that no
    /// longer passes its checks.
    fn webhook_text(
        &self,
        webhook: &IncomingWebhookDto,
        headers: &[(String, String)],
        body: &[u8],
    ) -> ServiceResult<Option<String>>;
}
