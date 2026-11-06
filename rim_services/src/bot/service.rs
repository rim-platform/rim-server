use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::bot::model::BotTokenId;
use rim_core::conversation::model::ConversationId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use crate::bot::dto::{
    BotActorDto, BotDto, BotInfoDto, BotMentionCursor, BotMentionsPage, BotTokenDto, CreatedBotDto,
    CreatedBotTokenDto,
};
use crate::{Actor, ServiceResult};

/// Bot accounts of a workspace and their tokens.
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
    ) -> ServiceResult<CreatedBotDto>;

    /// Change the fields given. Owner or Moderator.
    async fn update_bot(
        &self,
        actor: Actor,
        user_id: UserId,
        name: Option<String>,
        run_by: Option<String>,
        reads_go_to: Option<String>,
    ) -> ServiceResult<BotDto>;

    /// Deactivate a bot: its tokens stop working and it leaves the workspace and its channels. Its
    /// messages stay. Doing it again changes nothing. Owner or Moderator.
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

    /// Record in the audit log a read of the bot that ran without the command dispatcher, with the
    /// initiator `bot`. `error` is what it failed with. A failed write is logged, never returned.
    async fn record_read(
        &self,
        bot: BotActorDto,
        command_id: &'static str,
        args: serde_json::Value,
        error: Option<String>,
    );
}
