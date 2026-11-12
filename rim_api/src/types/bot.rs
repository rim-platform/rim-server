use async_graphql::dataloader::DataLoader;
use async_graphql::{Context, Enum, ID, InputObject, Json, Result, SimpleObject};
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
    /// Whether an Owner or Moderator let it see members joining the workspace.
    pub sees_member_joins: bool,
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
            sees_member_joins: value.sees_member_joins,
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
    /// Whether it sees members joining the workspace; off when left out.
    pub sees_member_joins: Option<bool>,
}

/// Change a bot; a field left out stays as it is.
#[derive(InputObject)]
pub struct UpdateBotInput {
    pub bot_user_id: ID,
    pub name: Option<String>,
    pub run_by: Option<String>,
    pub reads_go_to: Option<String>,
    /// Whether it sees members joining the workspace.
    pub sees_member_joins: Option<bool>,
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
    /// Its own format; null for a webhook that follows its preset, such as an unedited GitHub one.
    pub template: Option<Json<serde_json::Value>>,
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
            template: value.template.map(Json),
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
    /// it is a secret. The empty string when `keptTestUrl` is true: the client already has the URL,
    /// and the server held only its hash.
    pub token: String,
    /// Whether the webhook kept the token of the test session named in `testSessionId`.
    pub kept_test_url: bool,
}

impl From<rim_services::bot::dto::CreatedIncomingWebhookDto> for CreatedIncomingWebhook {
    fn from(value: rim_services::bot::dto::CreatedIncomingWebhookDto) -> Self {
        Self {
            webhook: value.webhook.into(),
            token: value.token,
            kept_test_url: value.kept_test_url,
        }
    }
}

/// A webhook test session just started.
#[derive(SimpleObject, Clone)]
pub struct WebhookTestSession {
    pub id: ID,
    /// `rimhook_…` for an unbound session's URL, shown this once; null for a bound session.
    pub token: Option<String>,
    pub expires_at: DateTime<Utc>,
}

impl From<rim_services::bot::dto::StartedTestSessionDto> for WebhookTestSession {
    fn from(value: rim_services::bot::dto::StartedTestSessionDto) -> Self {
        Self {
            id: ID(value.id.to_string()),
            token: value.token,
            expires_at: value.expires_at,
        }
    }
}

/// What a test session holds of a delivery's body.
#[derive(Enum, Copy, Clone, Debug, Eq, PartialEq)]
pub enum WebhookTestBodyState {
    /// `body` is the body exactly as it came.
    Text,
    /// Over 256 KiB; its bytes are not held.
    TooLarge,
    /// Not UTF-8; its bytes are not held.
    NotText,
}

/// A delivery a test session holds.
#[derive(SimpleObject, Clone)]
pub struct WebhookTestDelivery {
    /// Its place in the session, counted from 1; pass the last one read as `after`.
    pub id: i32,
    pub received_at: DateTime<Utc>,
    pub content_type: Option<String>,
    /// The `x-` headers a format may use.
    pub headers: Vec<WebhookHeader>,
    pub body_state: WebhookTestBodyState,
    /// The body, for `TEXT` alone. A client shows it as plain text.
    pub body: Option<String>,
}

/// The deliveries a read of a test session answers, and when it now expires.
#[derive(SimpleObject, Clone)]
pub struct WebhookTestDeliveries {
    pub deliveries: Vec<WebhookTestDelivery>,
    pub expires_at: DateTime<Utc>,
}

impl From<rim_services::bot::dto::TestDeliveriesDto> for WebhookTestDeliveries {
    fn from(value: rim_services::bot::dto::TestDeliveriesDto) -> Self {
        use rim_services::bot::test_session::TestBody;
        Self {
            deliveries: value
                .deliveries
                .into_iter()
                .map(|delivery| {
                    let (body_state, body) = match delivery.body {
                        TestBody::Text(text) => (WebhookTestBodyState::Text, Some(text)),
                        TestBody::TooLarge => (WebhookTestBodyState::TooLarge, None),
                        TestBody::NotText => (WebhookTestBodyState::NotText, None),
                    };
                    WebhookTestDelivery {
                        id: i32::try_from(delivery.seq).unwrap_or(i32::MAX),
                        received_at: delivery.received_at,
                        content_type: delivery.content_type,
                        headers: delivery
                            .headers
                            .into_iter()
                            .map(|(name, value)| WebhookHeader { name, value })
                            .collect(),
                        body_state,
                        body,
                    }
                })
                .collect(),
            expires_at: value.expires_at,
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
    /// The webhook's own format, with the preset `PLAIN`. Sent only to a server that lists the
    /// capability `webhook_formats`.
    pub template: Option<Json<serde_json::Value>>,
    /// The caller's unbound test session whose URL the webhook keeps. Sent only to a server that
    /// lists the capability `webhook_test_sessions`.
    pub test_session_id: Option<ID>,
}

/// A header of a sample delivery.
#[derive(SimpleObject, InputObject, Clone, Debug)]
#[graphql(input_name = "WebhookHeaderInput")]
pub struct WebhookHeader {
    pub name: String,
    pub value: String,
}

/// A ready-made webhook format RIM ships, the same for everyone.
#[derive(SimpleObject, Clone, Debug)]
pub struct WebhookFormatInfo {
    /// A short name that stays the same, such as `github`.
    pub key: String,
    pub name: String,
    /// What to set up on the service's side.
    pub setup_note: String,
    pub format: Json<serde_json::Value>,
    pub sample_headers: Vec<WebhookHeader>,
    /// A delivery's body as the service sends it.
    pub sample_body: String,
}

impl From<rim_services::bot::dto::WebhookFormatInfoDto> for WebhookFormatInfo {
    fn from(value: rim_services::bot::dto::WebhookFormatInfoDto) -> Self {
        Self {
            key: value.key,
            name: value.name,
            setup_note: value.setup_note,
            format: Json(value.format),
            sample_headers: value
                .sample_headers
                .into_iter()
                .map(|(name, value)| WebhookHeader { name, value })
                .collect(),
            sample_body: value.sample_body,
        }
    }
}

/// What a sample delivery would come to.
#[derive(Enum, Copy, Clone, Debug, Eq, PartialEq)]
pub enum WebhookPreviewOutcome {
    /// A rule holds and posts.
    Posts,
    /// No rule holds, so nothing would post.
    NoRule,
    /// The rule that holds has a placeholder with no value, so nothing would post.
    NoValue,
    /// The body would be refused.
    BodyRefused,
    /// The format would be refused.
    FormatRefused,
}

/// What a sample delivery would come to under a format. Nothing is posted or stored.
#[derive(SimpleObject, Clone, Debug)]
pub struct WebhookFormatPreview {
    pub outcome: WebhookPreviewOutcome,
    /// The rule that decides or that a refusal is about, counted from 1.
    pub rule: Option<i32>,
    /// What would post, without the link.
    pub text: Option<String>,
    /// The link after the text, when it passes its check.
    pub link: Option<String>,
    /// The placeholder with no value, as written between its braces.
    pub placeholder: Option<String>,
    /// Why the body or the format would be refused.
    pub reason: Option<String>,
}

impl From<rim_services::bot::dto::WebhookPreviewDto> for WebhookFormatPreview {
    fn from(value: rim_services::bot::dto::WebhookPreviewDto) -> Self {
        use rim_services::bot::dto::WebhookPreviewDto as Dto;
        let rule = |rule: usize| i32::try_from(rule).ok();
        let empty = |outcome| Self {
            outcome,
            rule: None,
            text: None,
            link: None,
            placeholder: None,
            reason: None,
        };
        match value {
            Dto::Posts {
                rule: at,
                text,
                link,
            } => Self {
                rule: rule(at),
                text: Some(text),
                link,
                ..empty(WebhookPreviewOutcome::Posts)
            },
            Dto::NoRule => empty(WebhookPreviewOutcome::NoRule),
            Dto::NoValue {
                rule: at,
                placeholder,
            } => Self {
                rule: rule(at),
                placeholder: Some(placeholder),
                ..empty(WebhookPreviewOutcome::NoValue)
            },
            Dto::BodyRefused { reason } => Self {
                reason: Some(reason),
                ..empty(WebhookPreviewOutcome::BodyRefused)
            },
            Dto::FormatRefused { rule: at, reason } => Self {
                rule: at.and_then(rule),
                reason: Some(reason),
                ..empty(WebhookPreviewOutcome::FormatRefused)
            },
        }
    }
}
