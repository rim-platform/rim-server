use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Enum, ID, InputObject, Result, SimpleObject};
use chrono::{DateTime, Utc};
use rim_core::user::model::UserId;

use crate::ids::parse_uuid_id;
use crate::loaders::BotInfoLoader;

/// What the person who added a bot says about it. RIM cannot check it.
#[derive(SimpleObject, Clone, Debug)]
pub struct BotInfo {
    /// Who runs it.
    pub run_by: String,
    /// Where what it reads goes.
    pub reads_go_to: String,
    /// Who added it.
    pub added_by_name: Option<String>,
}

impl From<rim_services::bot::dto::BotInfoDto> for BotInfo {
    fn from(value: rim_services::bot::dto::BotInfoDto) -> Self {
        Self {
            run_by: value.run_by,
            reads_go_to: value.reads_go_to,
            added_by_name: value.added_by_name,
        }
    }
}

/// [`BotInfo`] of the user `id`, loaded in a batch; null without a query for anyone but a bot.
pub(crate) async fn load_bot_info(
    ctx: &Context<'_>,
    is_bot: bool,
    id: &ID,
) -> Result<Option<BotInfo>> {
    if !is_bot {
        return Ok(None);
    }
    let user_id = parse_uuid_id::<UserId>(id)?;
    let info = ctx
        .data::<DataLoader<BotInfoLoader>>()?
        .load_one(user_id)
        .await
        .map_err(|e| (*e).clone())?;
    Ok(info.map(Into::into))
}

/// A bot account of a workspace.
#[derive(SimpleObject, Clone, Debug)]
pub struct Bot {
    pub user_id: ID,
    pub server_id: ID,
    pub name: String,
    /// Who runs it, as the person who added it says.
    pub run_by: String,
    /// Where what it reads goes, as the person who added it says.
    pub reads_go_to: String,
    /// Who added it.
    pub added_by_name: Option<String>,
    pub created_at: DateTime<Utc>,
    /// When it was deactivated; null while it is active.
    pub deactivated_at: Option<DateTime<Utc>>,
}

impl From<rim_services::bot::dto::BotDto> for Bot {
    fn from(value: rim_services::bot::dto::BotDto) -> Self {
        Self {
            user_id: ID(value.user_id.to_string()),
            server_id: ID(value.server_id.to_string()),
            name: value.name,
            run_by: value.run_by,
            reads_go_to: value.reads_go_to,
            added_by_name: value.added_by_name,
            created_at: value.created_at,
            deactivated_at: value.deactivated_at,
        }
    }
}

/// A bot's token, without its secret, which is shown once when it is created.
#[derive(SimpleObject, Clone, Debug)]
pub struct BotToken {
    pub id: ID,
    pub label: String,
    pub created_at: DateTime<Utc>,
    /// When it was last used, to the minute; null when it never was.
    pub last_used_at: Option<DateTime<Utc>>,
    /// When it was revoked; null while it works.
    pub revoked_at: Option<DateTime<Utc>>,
}

impl From<rim_services::bot::dto::BotTokenDto> for BotToken {
    fn from(value: rim_services::bot::dto::BotTokenDto) -> Self {
        Self {
            id: ID(value.id.to_string()),
            label: value.label,
            created_at: value.created_at,
            last_used_at: value.last_used_at,
            revoked_at: value.revoked_at,
        }
    }
}

/// A new bot and its first token.
#[derive(SimpleObject, Clone)]
pub struct CreatedBot {
    pub bot: Bot,
    /// `rimbot_…`, shown this once.
    pub token: String,
}

impl From<rim_services::bot::dto::CreatedBotDto> for CreatedBot {
    fn from(value: rim_services::bot::dto::CreatedBotDto) -> Self {
        Self {
            bot: value.bot.into(),
            token: value.token,
        }
    }
}

/// A new token of a bot.
#[derive(SimpleObject, Clone)]
pub struct CreatedBotToken {
    pub bot_token: BotToken,
    /// `rimbot_…`, shown this once.
    pub token: String,
}

impl From<rim_services::bot::dto::CreatedBotTokenDto> for CreatedBotToken {
    fn from(value: rim_services::bot::dto::CreatedBotTokenDto) -> Self {
        Self {
            bot_token: value.bot_token.into(),
            token: value.token,
        }
    }
}

/// Create a bot in a workspace.
#[derive(InputObject)]
pub struct CreateBotInput {
    pub server_id: ID,
    pub name: String,
    /// Who runs it: 1 to 200 characters of plain text.
    pub run_by: String,
    /// Where what it reads goes: 1 to 200 characters of plain text.
    pub reads_go_to: String,
}

/// Change a bot; a field left out stays as it is.
#[derive(InputObject)]
pub struct UpdateBotInput {
    pub bot_user_id: ID,
    pub name: Option<String>,
    pub run_by: Option<String>,
    pub reads_go_to: Option<String>,
}

/// How an incoming webhook's body becomes text.
#[derive(Enum, Copy, Clone, Debug, Eq, PartialEq)]
pub enum WebhookPreset {
    /// `{"text": "..."}`, at most 4,000 characters.
    Plain,
    /// GitHub's published releases, pull requests opened, closed or merged and issues opened or
    /// closed, by the `X-GitHub-Event` header.
    Github,
}

impl From<rim_core::bot::model::WebhookPreset> for WebhookPreset {
    fn from(value: rim_core::bot::model::WebhookPreset) -> Self {
        match value {
            rim_core::bot::model::WebhookPreset::Plain => Self::Plain,
            rim_core::bot::model::WebhookPreset::Github => Self::Github,
        }
    }
}

impl From<WebhookPreset> for rim_core::bot::model::WebhookPreset {
    fn from(value: WebhookPreset) -> Self {
        match value {
            WebhookPreset::Plain => Self::Plain,
            WebhookPreset::Github => Self::Github,
        }
    }
}

/// A URL that posts to one channel as one bot, without its token, which is shown once.
#[derive(SimpleObject, Clone, Debug)]
pub struct IncomingWebhook {
    pub id: ID,
    pub conversation_id: ID,
    /// The bot it posts as.
    pub bot_user_id: ID,
    pub preset: WebhookPreset,
    pub created_at: DateTime<Utc>,
    /// When it was last used, to the minute; null when it never was.
    pub last_used_at: Option<DateTime<Utc>>,
    /// When it was revoked; null until then.
    pub revoked_at: Option<DateTime<Utc>>,
}

impl From<rim_services::bot::dto::IncomingWebhookDto> for IncomingWebhook {
    fn from(value: rim_services::bot::dto::IncomingWebhookDto) -> Self {
        Self {
            id: ID(value.id.to_string()),
            conversation_id: ID(value.conversation_id.to_string()),
            bot_user_id: ID(value.bot_user_id.to_string()),
            preset: value.preset.into(),
            created_at: value.created_at,
            last_used_at: value.last_used_at,
            revoked_at: value.revoked_at,
        }
    }
}

/// A new incoming webhook and its token.
#[derive(SimpleObject, Clone)]
pub struct CreatedIncomingWebhook {
    pub webhook: IncomingWebhook,
    /// `rimhook_…`, shown this once. The URL is `/api/hooks/` and this on the server's address, and
    /// it is a secret.
    pub token: String,
}

impl From<rim_services::bot::dto::CreatedIncomingWebhookDto> for CreatedIncomingWebhook {
    fn from(value: rim_services::bot::dto::CreatedIncomingWebhookDto) -> Self {
        Self {
            webhook: value.webhook.into(),
            token: value.token,
        }
    }
}

/// Create a channel's incoming webhook.
#[derive(InputObject)]
pub struct CreateIncomingWebhookInput {
    pub conversation_id: ID,
    /// The bot it posts as, an active bot of the channel's workspace.
    pub bot_user_id: ID,
    pub preset: WebhookPreset,
}
