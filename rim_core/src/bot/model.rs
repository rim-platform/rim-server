//! Bot accounts of a workspace, their tokens and the incoming webhooks bound to them. A bot is a
//! `user` row of the kind `bot`, so it is keyed by its [`UserId`].

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::conversation::model::ConversationId;
use crate::message::model::MessageId;
use crate::message_thread::model::MessageThreadId;
use crate::model_id;
use crate::server::model::ServerId;
use crate::server_member::model::ServerMemberId;
use crate::user::model::UserId;

model_id!(BotTokenId);
model_id!(IncomingWebhookId);
model_id!(WebhookTestSessionId);

/// A bot as its workspace sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bot {
    pub user_id: UserId,
    pub server_id: ServerId,
    /// The bot's display name, from its `user` row.
    pub name: String,
    /// Who runs it, as the person who added it says.
    pub run_by: String,
    /// Where what it reads goes, as the person who added it says.
    pub reads_go_to: String,
    pub created_by: UserId,
    /// The name of the person who added it; `None` when that account has no name left to show.
    pub added_by_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub deactivated_at: Option<DateTime<Utc>>,
    /// From when an Owner or Moderator let it see members joining its workspace; `None` while they
    /// don't.
    pub member_joins_since: Option<DateTime<Utc>>,
}

impl Bot {
    /// Whether it may see members joining its workspace now.
    pub fn sees_member_joins(&self) -> bool {
        self.member_joins_since.is_some()
    }
}

/// A new bot, with what its `user` row is created with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBot {
    pub server_id: ServerId,
    pub username: String,
    pub name: String,
    pub run_by: String,
    pub reads_go_to: String,
    pub created_by: UserId,
    /// Whether it sees members joining its workspace, from its creation on.
    pub sees_member_joins: bool,
}

/// The fields of a bot to change; `None` leaves a field as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BotPatch {
    pub name: Option<String>,
    pub run_by: Option<String>,
    pub reads_go_to: Option<String>,
    /// Turning it on starts from now, and on a bot already on changes nothing. Turning it off
    /// forgets the start.
    pub sees_member_joins: Option<bool>,
}

/// A position in a bot's feed of events: a time and the id of the row at it. Rows of every kind
/// sort together by this pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BotFeedPosition {
    pub at: DateTime<Utc>,
    pub id: Uuid,
}

/// A message of a bot's feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotFeedMessage {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub thread_id: Option<MessageThreadId>,
    pub sent_by: UserId,
    pub text: Option<String>,
    pub sent_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    /// Whether the message mentions the bot reading it.
    pub mentioned: bool,
}

/// A member joining the workspace, in a bot's feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BotFeedJoin {
    pub member_id: ServerMemberId,
    pub user_id: UserId,
    pub joined_at: DateTime<Utc>,
}

/// A bot's token, never with its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotToken {
    pub id: BotTokenId,
    pub bot_user_id: UserId,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// A new token: the SHA-256 of the string shown once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBotToken {
    pub bot_user_id: UserId,
    pub label: String,
    pub token_hash: String,
    pub created_by: UserId,
}

/// A token found by its hash, with the bot's standing as far as the bot tables know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotTokenHolder {
    pub token: BotToken,
    pub server_id: ServerId,
    pub bot_deactivated_at: Option<DateTime<Utc>>,
}

/// How an incoming webhook's body becomes text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookPreset {
    /// A JSON object with a string `text`.
    Plain,
    /// GitHub's events, chosen by the `X-GitHub-Event` header.
    Github,
}

impl WebhookPreset {
    /// The value kept in the `preset` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Github => "github",
        }
    }

    /// The preset [`Self::as_str`] names, or `None` for anything else.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "plain" => Some(Self::Plain),
            "github" => Some(Self::Github),
            _ => None,
        }
    }
}

/// An incoming webhook of a channel, never with its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingWebhook {
    pub id: IncomingWebhookId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub bot_user_id: UserId,
    pub preset: WebhookPreset,
    /// The webhook's own format as it was saved, read only when a delivery comes; `None` for a
    /// webhook that follows its preset.
    pub template: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// A new incoming webhook: the SHA-256 of the token shown once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewIncomingWebhook {
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub bot_user_id: UserId,
    pub preset: WebhookPreset,
    /// Its own format, checked before it is stored; `None` to follow the preset.
    pub template: Option<serde_json::Value>,
    pub token_hash: String,
    pub created_by: UserId,
}

/// A webhook found by its hash, with its bot's standing as far as the bot tables know it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingWebhookHolder {
    pub webhook: IncomingWebhook,
    pub bot_deactivated_at: Option<DateTime<Utc>>,
}
