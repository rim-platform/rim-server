use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::bot::model::{
    Bot, BotFeedJoin, BotFeedMessage, BotFeedPosition, BotPatch, BotToken, BotTokenHolder,
    BotTokenId, IncomingWebhook, IncomingWebhookHolder, IncomingWebhookId, NewBot, NewBotToken,
    NewIncomingWebhook,
};
use crate::conversation::model::ConversationId;
use crate::repository::error::RepoResult;
use crate::server::model::ServerId;
use crate::user::model::UserId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait BotRepository: Send + Sync {
    /// Insert a `user` row of the kind `bot`, with no email, key or terms, and its `bot` row.
    ///
    /// # Errors
    /// `Conflict` when the username is taken.
    async fn create_bot(&self, bot: NewBot) -> RepoResult<Bot>;

    async fn find_bot(&self, user_id: UserId) -> RepoResult<Option<Bot>>;

    /// The workspace's bots, deactivated ones included, oldest first.
    async fn list_by_server(&self, server_id: ServerId) -> RepoResult<Vec<Bot>>;

    /// The bots among `user_ids`; other users are left out.
    async fn list_by_users(&self, user_ids: &[UserId]) -> RepoResult<Vec<Bot>>;

    /// The users among `user_ids` of the kind `bot`, whatever became of their `bot` row.
    async fn bots_among(&self, user_ids: &[UserId]) -> RepoResult<Vec<UserId>>;

    /// How many bots of the workspace are not deactivated.
    async fn count_active(&self, server_id: ServerId) -> RepoResult<u64>;

    /// Hold the workspace's bot lock until the surrounding transaction ends, so creations that
    /// check the workspace's cap run one after another.
    async fn lock_server_bots(&self, server_id: ServerId) -> RepoResult<()>;

    /// Lock the bot's row until the surrounding transaction ends, so a token creation and a
    /// deactivation run one after another. Returns `false` when there is no such bot.
    async fn lock_bot(&self, user_id: UserId) -> RepoResult<bool>;

    /// Change the fields `patch` names, the name on the bot's `user` row.
    ///
    /// # Errors
    /// `NotFound` when there is no such bot.
    async fn update_bot(&self, user_id: UserId, patch: BotPatch) -> RepoResult<Bot>;

    /// Stamp a bot not yet deactivated as deactivated at `at`. Returns whether that changed it.
    async fn set_deactivated(&self, user_id: UserId, at: DateTime<Utc>) -> RepoResult<bool>;

    async fn create_token(&self, token: NewBotToken) -> RepoResult<BotToken>;

    async fn find_token(&self, id: BotTokenId) -> RepoResult<Option<BotToken>>;

    /// The bot's tokens, revoked ones included, newest first.
    async fn list_tokens(&self, bot_user_id: UserId) -> RepoResult<Vec<BotToken>>;

    /// How many tokens of the bot are not revoked.
    async fn count_active_tokens(&self, bot_user_id: UserId) -> RepoResult<u64>;

    /// Stamp a token not yet revoked as revoked at `at`. Returns the token as it now is, or
    /// `None` when there is no such token.
    async fn revoke_token(&self, id: BotTokenId, at: DateTime<Utc>)
    -> RepoResult<Option<BotToken>>;

    /// Stamp every token of the bot not yet revoked as revoked at `at`. Returns how many.
    async fn revoke_all_tokens(&self, bot_user_id: UserId, at: DateTime<Utc>) -> RepoResult<u64>;

    /// The token with the SHA-256 `token_hash` and its bot's standing, by the unique index.
    async fn find_token_holder(&self, token_hash: &str) -> RepoResult<Option<BotTokenHolder>>;

    /// Record that a token was used at `at`.
    async fn touch_token(&self, id: BotTokenId, at: DateTime<Utc>) -> RepoResult<()>;

    /// Insert an incoming webhook.
    ///
    /// # Errors
    /// `Conflict` when its channel already has one that is not revoked.
    async fn create_webhook(&self, webhook: NewIncomingWebhook) -> RepoResult<IncomingWebhook>;

    async fn find_webhook(&self, id: IncomingWebhookId) -> RepoResult<Option<IncomingWebhook>>;

    /// The webhook of the channel that is not revoked, by the partial unique index.
    async fn find_active_webhook(
        &self,
        conversation_id: ConversationId,
    ) -> RepoResult<Option<IncomingWebhook>>;

    /// The workspace's webhooks, revoked ones included, newest first.
    async fn list_webhooks(&self, server_id: ServerId) -> RepoResult<Vec<IncomingWebhook>>;

    /// Stamp a webhook not yet revoked as revoked at `at`. Returns the webhook as it now is, or
    /// `None` when there is no such webhook.
    async fn revoke_webhook(
        &self,
        id: IncomingWebhookId,
        at: DateTime<Utc>,
    ) -> RepoResult<Option<IncomingWebhook>>;

    /// Stamp every webhook of the bot not yet revoked as revoked at `at`. Returns how many.
    async fn revoke_all_webhooks(&self, bot_user_id: UserId, at: DateTime<Utc>) -> RepoResult<u64>;

    /// The webhook with the SHA-256 `token_hash` and its bot's standing, by the unique index.
    async fn find_webhook_holder(
        &self,
        token_hash: &str,
    ) -> RepoResult<Option<IncomingWebhookHolder>>;

    /// Record that a webhook was used at `at`.
    async fn touch_webhook(&self, id: IncomingWebhookId, at: DateTime<Utc>) -> RepoResult<()>;

    /// The database's clock, which stamps messages and memberships.
    async fn feed_now(&self) -> RepoResult<DateTime<Utc>>;

    /// The messages of the bot's feed strictly after `after`, oldest first by `(sent_at, id)`, at
    /// most `limit`: top-level and not deleted, as a channel's message list has them, by someone
    /// else, sent since the bot joined the channel and at least `hold_back` old by the database's
    /// clock, in plaintext channels of `server_id` that are not archived and that it is in now.
    async fn list_feed_messages(
        &self,
        bot_user_id: UserId,
        server_id: ServerId,
        after: BotFeedPosition,
        hold_back: Duration,
        limit: u64,
    ) -> RepoResult<Vec<BotFeedMessage>>;

    /// The members of `server_id` who joined strictly after `after`, oldest first by
    /// `(joined_at, id)`, at most `limit`: anyone but the bot, since the bot was allowed to see them
    /// and at least `hold_back` old by the database's clock. Nothing while it isn't allowed.
    async fn list_feed_joins(
        &self,
        bot_user_id: UserId,
        server_id: ServerId,
        after: BotFeedPosition,
        hold_back: Duration,
        limit: u64,
    ) -> RepoResult<Vec<BotFeedJoin>>;
}
