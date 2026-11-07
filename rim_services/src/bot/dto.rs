use std::fmt;

use chrono::{DateTime, Utc};
use rim_core::bot::model::{
    Bot, BotFeedMessage, BotFeedPosition, BotToken, BotTokenId, IncomingWebhook, IncomingWebhookId,
    WebhookPreset,
};
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
    pub sees_member_joins: bool,
}

impl From<Bot> for BotDto {
    fn from(bot: Bot) -> Self {
        Self {
            sees_member_joins: bot.sees_member_joins(),
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

impl From<BotFeedMessage> for BotMessageDto {
    fn from(message: BotFeedMessage) -> Self {
        Self {
            id: message.id,
            conversation_id: message.conversation_id,
            thread_id: message.thread_id,
            author_id: message.sent_by,
            text: message.text,
            sent_at: message.sent_at,
            edited_at: message.edited_at,
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

/// One thing a bot's feed tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BotEventDto {
    /// A message in one of its channels. `mentioned` says whether it mentions the bot.
    Message {
        message: BotMessageDto,
        mentioned: bool,
    },
    /// A member joining its workspace.
    MemberJoined { user_id: UserId, at: DateTime<Utc> },
}

/// One page of a bot's feed, oldest first, and where the next call goes on from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotEventsPage {
    pub events: Vec<BotEventDto>,
    pub cursor: BotFeedPosition,
}

/// An incoming webhook as its workspace's Owner and Moderators see it. Never its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingWebhookDto {
    pub id: IncomingWebhookId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    pub bot_user_id: UserId,
    pub preset: WebhookPreset,
    /// Its own format; `None` for one that follows its preset.
    pub template: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl From<IncomingWebhook> for IncomingWebhookDto {
    fn from(webhook: IncomingWebhook) -> Self {
        Self {
            id: webhook.id,
            server_id: webhook.server_id,
            conversation_id: webhook.conversation_id,
            bot_user_id: webhook.bot_user_id,
            preset: webhook.preset,
            template: webhook.template,
            created_at: webhook.created_at,
            last_used_at: webhook.last_used_at,
            revoked_at: webhook.revoked_at,
        }
    }
}

/// The most characters of a delivery's header value a format reads; a longer one counts as missing.
pub const MAX_HEADER_VALUE_CHARS: usize = 64;
/// The most `x-` headers of a delivery a format reads.
pub const MAX_DELIVERY_HEADERS: usize = 50;

/// The headers of a delivery a format reads, from `(name, value)` pairs: names starting `x-`, in
/// lowercase, with a value of at most [`MAX_HEADER_VALUE_CHARS`], and at most
/// [`MAX_DELIVERY_HEADERS`] of them. A preview reads a sample the same way.
pub fn delivery_headers<'a>(
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Vec<(String, String)> {
    headers
        .into_iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value))
        .filter(|(name, value)| {
            name.starts_with("x-") && value.chars().count() <= MAX_HEADER_VALUE_CHARS
        })
        .take(MAX_DELIVERY_HEADERS)
        .map(|(name, value)| (name, value.to_string()))
        .collect()
}

/// A ready-made webhook format RIM ships, the same for everyone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookFormatInfoDto {
    pub key: String,
    pub name: String,
    /// What to set up on the service's side.
    pub setup_note: String,
    pub format: serde_json::Value,
    pub sample_headers: Vec<(String, String)>,
    pub sample_body: String,
}

/// What a sample delivery would come to under a format. Rules are counted from 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookPreviewDto {
    /// Rule `rule` holds and posts `text`, then `link` when there is one.
    Posts {
        rule: usize,
        text: String,
        link: Option<String>,
    },
    /// No rule holds, so nothing would post.
    NoRule,
    /// Rule `rule` holds, but `placeholder` has no value, so nothing would post.
    NoValue { rule: usize, placeholder: String },
    /// The body would be refused with `reason`.
    BodyRefused { reason: String },
    /// The format would be refused with `reason`, about rule `rule` when it names one.
    FormatRefused { rule: Option<usize>, reason: String },
}

/// A new incoming webhook and its token, shown this once. `Debug` leaves the token out.
#[derive(Clone)]
pub struct CreatedIncomingWebhookDto {
    pub webhook: IncomingWebhookDto,
    pub token: String,
}

impl fmt::Debug for CreatedIncomingWebhookDto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedIncomingWebhookDto")
            .field("webhook", &self.webhook)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delivery_keeps_its_short_x_headers_in_lowercase_and_nothing_else() {
        let long = "e".repeat(MAX_HEADER_VALUE_CHARS + 1);
        let limit = "e".repeat(MAX_HEADER_VALUE_CHARS);
        let kept = delivery_headers([
            ("X-GitHub-Event", "push"),
            ("authorization", "Bearer secret"),
            ("cookie", "a=b"),
            ("content-type", "application/json"),
            ("x-long", long.as_str()),
            ("x-limit", limit.as_str()),
        ]);
        assert_eq!(
            kept,
            [
                ("x-github-event".to_string(), "push".to_string()),
                ("x-limit".to_string(), limit.clone()),
            ]
        );
    }

    #[test]
    fn a_delivery_keeps_at_most_fifty_headers() {
        let names: Vec<String> = (0..60).map(|n| format!("x-h{n}")).collect();
        let kept = delivery_headers(names.iter().map(|name| (name.as_str(), "v")));
        assert_eq!(kept.len(), MAX_DELIVERY_HEADERS);
        assert_eq!(kept[0].0, "x-h0");
    }
}
