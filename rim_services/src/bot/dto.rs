use std::fmt;

use chrono::{DateTime, Utc};
use rim_core::bot::model::{Bot, BotToken, BotTokenId};
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::{Message, MessageId};
use rim_core::message_thread::model::MessageThreadId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use crate::message::dto::MessageDto;

/// A bot as a workspace's members see it.
#[derive(Debug, Clone)]
pub struct BotDto {
    pub user_id: UserId,
    pub server_id: ServerId,
    pub name: String,
    pub run_by: String,
    pub reads_go_to: String,
    pub added_by_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub deactivated_at: Option<DateTime<Utc>>,
}

impl From<Bot> for BotDto {
    fn from(bot: Bot) -> Self {
        Self {
            user_id: bot.user_id,
            server_id: bot.server_id,
            name: bot.name,
            run_by: bot.run_by,
            reads_go_to: bot.reads_go_to,
            added_by_name: bot.added_by_name,
            created_at: bot.created_at,
            deactivated_at: bot.deactivated_at,
        }
    }
}

/// What everyone who can see a bot sees about it: what the person who added it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotInfoDto {
    pub user_id: UserId,
    pub run_by: String,
    pub reads_go_to: String,
    pub added_by_name: Option<String>,
}

impl From<Bot> for BotInfoDto {
    fn from(bot: Bot) -> Self {
        Self {
            user_id: bot.user_id,
            run_by: bot.run_by,
            reads_go_to: bot.reads_go_to,
            added_by_name: bot.added_by_name,
        }
    }
}

/// A bot's token as its workspace's Owner and Moderators see it. Never its hash.
#[derive(Debug, Clone)]
pub struct BotTokenDto {
    pub id: BotTokenId,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl From<BotToken> for BotTokenDto {
    fn from(token: BotToken) -> Self {
        Self {
            id: token.id,
            label: token.label,
            created_at: token.created_at,
            last_used_at: token.last_used_at,
            revoked_at: token.revoked_at,
        }
    }
}

/// A new bot and its first token, shown this once. `Debug` leaves the token out.
#[derive(Clone)]
pub struct CreatedBotDto {
    pub bot: BotDto,
    pub token: String,
}

impl fmt::Debug for CreatedBotDto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedBotDto")
            .field("bot", &self.bot)
            .finish_non_exhaustive()
    }
}

/// A new token, shown this once. `Debug` leaves the token out.
#[derive(Clone)]
pub struct CreatedBotTokenDto {
    pub bot_token: BotTokenDto,
    pub token: String,
}

impl fmt::Debug for CreatedBotTokenDto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedBotTokenDto")
            .field("bot_token", &self.bot_token)
            .finish_non_exhaustive()
    }
}

/// Who a valid bot token acts as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BotActorDto {
    pub user_id: UserId,
    pub server_id: ServerId,
    pub token_id: BotTokenId,
}

/// A message as a bot reads it. `text` is `None` for a deleted message, and so is `edited_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotMessageDto {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub thread_id: Option<MessageThreadId>,
    pub author_id: UserId,
    pub text: Option<String>,
    pub sent_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
}

impl From<MessageDto> for BotMessageDto {
    fn from(message: MessageDto) -> Self {
        Self {
            id: message.id,
            conversation_id: message.conversation_id,
            thread_id: message.thread_id,
            author_id: message.author_id,
            text: message.text.filter(|_| !message.is_deleted),
            sent_at: message.sent_at,
            edited_at: message.edited_at.filter(|_| !message.is_deleted),
        }
    }
}

impl From<Message> for BotMessageDto {
    fn from(message: Message) -> Self {
        Self {
            id: message.id,
            conversation_id: message.conversation_id,
            thread_id: message.thread_id,
            author_id: message.sent_by,
            text: message.text.filter(|_| !message.is_deleted),
            sent_at: message.sent_at,
            edited_at: message.edited_at.filter(|_| !message.is_deleted),
        }
    }
}

/// Where a bot's reading of its mentions got to: the last mention it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BotMentionCursor {
    pub created_at: DateTime<Utc>,
    pub message_id: MessageId,
}

/// One page of a bot's mentions, oldest first, and the cursor of its last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotMentionsPage {
    pub messages: Vec<BotMessageDto>,
    /// `None` when the page is empty.
    pub last: Option<BotMentionCursor>,
}
