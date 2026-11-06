//! The bot API under `/api/bot/v1`, the only routes a bot token works on: who the bot is, the
//! channels it was added to, reading, searching and posting in them, and polling for its mentions.
//!
//! A request is authorized by its `Authorization: Bearer` header alone; a session cookie means
//! nothing here. Each route runs a fixed command with the initiator `Bot` or, where a command does
//! not give what the route answers, a read of the service behind it with its own audit entry.
//! Text somebody else wrote is marked `"untrusted": true` on every answer holding any.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use actix_web::http::{StatusCode, header};
use actix_web::{HttpRequest, HttpResponse, ResponseError, web};
use chrono::{DateTime, Utc};
use rim_core::conversation::model::ConversationId;
use rim_core::conversation::repository::ChannelListFilter;
use rim_core::message::model::MessageId;
use rim_core::repository::pagination::PageDir;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_infra_services::KeyedRateLimiter;
use rim_services::bot::dto::{BotActorDto, BotMentionCursor, BotMessageDto};
use rim_services::bot::service::BotService;
use rim_services::command::dto::{CommandContextDto, CommandResultStatusDto};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::command::refusal::{self, Refusal};
use rim_services::command::request::ExecuteCommandCmd;
use rim_services::command::service::CommandService;
use rim_services::conversation::command::ListChannelsQuery;
use rim_services::conversation::service::ConversationService;
use rim_services::message::command::ListMessagesQuery;
use rim_services::message::dto::MessagePageCursor;
use rim_services::message::search_query::parse_search_query;
use rim_services::message::service::MessageService;
use rim_services::provider::AppServiceProvider;
use rim_services::user::dto::UserDto;
use rim_services::user::service::UserService;
use rim_services::{Actor, ServiceError, ServiceResult};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::body::{BodyError, read_bounded};

/// The window of a token's call limit, in seconds.
const TOKEN_WINDOW_SECS: u64 = 60;
/// The calls one token may make in [`TOKEN_WINDOW_SECS`].
const TOKEN_CALLS_PER_WINDOW: u32 = 120;
/// The largest request body, in bytes.
const MAX_BODY_BYTES: usize = 128 * 1024;
/// The page size when a call does not name one.
const DEFAULT_LIMIT: u32 = 50;
/// The largest page size a call may name.
const MAX_LIMIT: u32 = 100;
/// The most characters of a search.
const MAX_QUERY_CHARS: usize = 500;
/// The channels read from the service at a time.
const CHANNELS_PER_PAGE: u32 = 100;

/// The command a post runs, from the bot API and from an incoming webhook.
pub(crate) const COMMAND_POST: &str = "chat.post_message";
const COMMAND_SEARCH: &str = "chat.search_messages";
const COMMAND_RECENT: &str = "chat.get_recent_messages";
const COMMAND_CHANNELS: &str = "chat.list_channels";
const COMMAND_ME: &str = "bot.me";

/// What the routes of the bot API use, shared by the server's workers.
pub(crate) struct BotApi {
    bots: Arc<dyn BotService>,
    commands: Arc<dyn CommandService>,
    messages: Arc<dyn MessageService>,
    conversations: Arc<dyn ConversationService>,
    users: Arc<dyn UserService>,
    /// Each token's calls per minute, charged after the token is known to work.
    token_calls: KeyedRateLimiter,
}

impl BotApi {
    pub(crate) fn new(provider: &dyn AppServiceProvider) -> Self {
        Self {
            bots: provider.bot_service(),
            commands: provider.command_service(),
            messages: provider.message_service(),
            conversations: provider.conversation_service(),
            users: provider.user_service(),
            token_calls: KeyedRateLimiter::new(TOKEN_WINDOW_SECS, TOKEN_CALLS_PER_WINDOW),
        }
    }

    /// The bot `req`'s bearer token acts as, after charging the token's call limit.
    ///
    /// # Errors
    /// `401` with the same bytes for a missing header and every token that does not work, `429`
    /// over the limit, `5xx` when the token could not be checked.
    async fn authorize(&self, req: &HttpRequest) -> Result<BotActorDto, BotApiError> {
        let token = bearer_token(req).ok_or_else(BotApiError::bad_token)?;
        let bot = self
            .bots
            .authenticate_token(token)
            .await?
            .ok_or_else(BotApiError::bad_token)?;
        let key = bot.token_id.to_string();
        match self.token_calls.check_and_record(&key, Utc::now()) {
            RateLimitDecision::Allowed => Ok(bot),
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => {
                rim_infra_services::metrics::rate_limited("bot_api");
                Err(BotApiError::rate_limited(retry_after_seconds))
            }
        }
    }

    /// The channel `raw` names, when the bot was added to it.
    ///
    /// # Errors
    /// The same `404` for a malformed id, an unknown one and a conversation the bot is not in.
    async fn joined_channel(
        &self,
        bot: BotActorDto,
        raw: &str,
    ) -> Result<ConversationId, BotApiError> {
        let id = Uuid::parse_str(raw).map_err(|_| BotApiError::not_found())?;
        let id = ConversationId::from(id);
        if self.bots.is_in_channel(bot, id).await? {
            Ok(id)
        } else {
            Err(BotApiError::not_found())
        }
    }

    /// Run the builtin `command_id` for `bot`, in its workspace and `channel`, and answer its
    /// payload.
    ///
    /// # Errors
    /// What the command's gates or the command itself refuse with; `403` for a command needing a
    /// confirmation, which a bot cannot give.
    async fn run(
        &self,
        bot: BotActorDto,
        command_id: &'static str,
        args: Value,
        channel: ConversationId,
    ) -> Result<Value, BotApiError> {
        run_command(
            self.commands.as_ref(),
            bot.user_id,
            bot.server_id,
            command_id,
            args,
            channel,
        )
        .await
    }

    /// Write the audit entry of a read that did not go through a command, and hand back `result`.
    async fn recorded<T>(
        &self,
        bot: BotActorDto,
        command_id: &'static str,
        args: Value,
        result: ServiceResult<T>,
    ) -> ServiceResult<T> {
        let error = result.as_ref().err().map(ToString::to_string);
        self.bots.record_read(bot, command_id, args, error).await;
        result
    }

    /// `messages` as their JSON, with their authors looked up in one batch.
    async fn messages_json(&self, messages: Vec<BotMessageDto>) -> Result<Vec<Value>, BotApiError> {
        let author_ids: Vec<UserId> = messages
            .iter()
            .map(|m| m.author_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let authors: HashMap<UserId, UserDto> = if author_ids.is_empty() {
            HashMap::new()
        } else {
            self.users
                .get_users_by_ids(&author_ids)
                .await?
                .into_iter()
                .map(|user| (user.id, user))
                .collect()
        };
        Ok(messages
            .iter()
            .map(|message| message_json(message, authors.get(&message.author_id)))
            .collect())
    }
}

/// Run the builtin `command_id` as the bot `user_id`, in its workspace `server_id` and `channel`,
/// and answer its payload.
///
/// # Errors
/// What the command's gates or the command itself refuse with; `403` for a command needing a
/// confirmation, which a bot cannot give.
pub(crate) async fn run_command(
    commands: &dyn CommandService,
    user_id: UserId,
    server_id: ServerId,
    command_id: &'static str,
    args: Value,
    channel: ConversationId,
) -> Result<Value, BotApiError> {
    let result = commands
        .execute_command(ExecuteCommandCmd {
            actor: Actor::new(user_id),
            command_id: command_id.to_string(),
            args: Some(args),
            context: Some(CommandContextDto {
                workspace_id: Some(server_id),
                conversation_id: Some(channel),
                thread_id: None,
                selected_text: None,
                selected_message_id: None,
                selected_message_ids: vec![],
                focused_surface: None,
                active_panel: None,
            }),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Bot,
            run_id: None,
        })
        .await?;
    match result.status {
        CommandResultStatusDto::Success => Ok(result.payload.unwrap_or(Value::Null)),
        CommandResultStatusDto::ConfirmationRequired => Err(BotApiError::denied(
            "This needs a person to confirm it, and a bot cannot.".to_string(),
        )),
        CommandResultStatusDto::Error => {
            tracing::error!(command_id, "a bot's command ended in an error");
            Err(BotApiError::failed(false))
        }
    }
}

/// A refused call, in the one shape every refusal of the bot API has.
#[derive(Debug)]
pub(crate) struct BotApiError {
    status: StatusCode,
    refusal: Refusal,
    message: String,
    retry_safe: bool,
    retry_after_seconds: Option<u64>,
}

impl BotApiError {
    pub(crate) fn new(
        status: StatusCode,
        refusal: Refusal,
        message: &str,
        retry_safe: bool,
    ) -> Self {
        Self {
            status,
            refusal,
            message: message.to_string(),
            retry_safe,
            retry_after_seconds: None,
        }
    }

    /// The `401` of every token that does not work.
    fn bad_token() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            Refusal::Denied,
            "This token isn't valid.",
            false,
        )
    }

    /// The `404` of everything the bot cannot see.
    fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            Refusal::Invalid,
            "There is nothing here.",
            false,
        )
    }

    pub(crate) fn invalid(message: &str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, Refusal::Invalid, message, false)
    }

    fn denied(message: String) -> Self {
        Self::new(StatusCode::FORBIDDEN, Refusal::Denied, &message, false)
    }

    pub(crate) fn rate_limited(retry_after_seconds: u64) -> Self {
        Self {
            retry_after_seconds: Some(retry_after_seconds),
            ..Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                Refusal::Failed,
                "Too many calls. Try again later.",
                true,
            )
        }
    }

    fn failed(retry_safe: bool) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            Refusal::Failed,
            "RIM could not do this.",
            retry_safe,
        )
    }
}

impl fmt::Display for BotApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<ServiceError> for BotApiError {
    fn from(err: ServiceError) -> Self {
        match (&err, Refusal::of(&err)) {
            (ServiceError::NotFound, _) => Self::not_found(),
            (
                ServiceError::RateLimited {
                    retry_after_seconds,
                },
                _,
            ) => Self::rate_limited(*retry_after_seconds),
            (ServiceError::Validation(message) | ServiceError::Conflict(message), _) => {
                Self::invalid(message)
            }
            (_, Some(kind @ (Refusal::Denied | Refusal::Expired))) => Self {
                refusal: kind,
                ..Self::denied(denied_message(&err))
            },
            _ => {
                tracing::error!(error = %err, "a bot API call failed");
                Self::failed(refusal::retry_safe(&err).unwrap_or(false))
            }
        }
    }
}

/// What a bot is told about a decision against it.
fn denied_message(err: &ServiceError) -> String {
    match err {
        ServiceError::Forbidden(reason) => reason.clone(),
        ServiceError::CapabilityMissing { .. } => err.to_string(),
        ServiceError::TimedOut { until } => {
            format!("This bot is timed out until {}.", until.to_rfc3339())
        }
        ServiceError::ConversationArchived => "This channel is archived.".to_string(),
        ServiceError::FeatureDisabled(_) => "This is switched off on this server.".to_string(),
        _ => "This bot may not do this.".to_string(),
    }
}

impl ResponseError for BotApiError {
    fn status_code(&self) -> StatusCode {
        self.status
    }

    fn error_response(&self) -> HttpResponse {
        let mut response = HttpResponse::build(self.status);
        if self.status == StatusCode::UNAUTHORIZED {
            response.insert_header((header::WWW_AUTHENTICATE, "Bearer"));
        }
        if let Some(seconds) = self.retry_after_seconds {
            response.insert_header((header::RETRY_AFTER, seconds.to_string()));
        }
        response.json(json!({
            "refusal": self.refusal.as_str(),
            "message": self.message,
            "retrySafe": self.retry_safe,
        }))
    }
}

/// The token of `req`'s `Authorization: Bearer` header, the only place a token is read from.
fn bearer_token(req: &HttpRequest) -> Option<&str> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then_some(token)
}

/// The query string of a call. Every field is optional, and a route reads the ones it takes.
#[derive(Debug, Deserialize)]
struct CallQuery {
    before: Option<String>,
    after: Option<String>,
    limit: Option<String>,
    q: Option<String>,
}

fn call_query(req: &HttpRequest) -> Result<CallQuery, BotApiError> {
    web::Query::<CallQuery>::from_query(req.query_string())
        .map(web::Query::into_inner)
        .map_err(|_| BotApiError::invalid("The query string isn't valid."))
}

/// A page size from 1 to [`MAX_LIMIT`], or [`DEFAULT_LIMIT`] when absent or empty.
fn page_limit(raw: Option<&str>) -> Result<u32, BotApiError> {
    match raw.filter(|raw| !raw.is_empty()) {
        None => Ok(DEFAULT_LIMIT),
        Some(raw) => raw
            .parse::<u32>()
            .ok()
            .filter(|limit| (1..=MAX_LIMIT).contains(limit))
            .ok_or_else(|| BotApiError::invalid("limit must be a number from 1 to 100.")),
    }
}

/// A cursor: the position's time in microseconds and its id, which a bot only passes back.
fn encode_cursor(at: DateTime<Utc>, id: Uuid) -> String {
    format!("{}_{}", at.timestamp_micros(), id.simple())
}

/// The time and id of a cursor from [`encode_cursor`], or `None` for anything else.
fn decode_cursor(raw: &str) -> Option<(DateTime<Utc>, Uuid)> {
    let (micros, id) = raw.split_once('_')?;
    let at = DateTime::from_timestamp_micros(micros.parse().ok()?)?;
    Some((at, Uuid::parse_str(id).ok()?))
}

/// The position `raw` names, with no position for an absent or empty one.
fn cursor_param(raw: Option<&str>) -> Result<Option<(DateTime<Utc>, Uuid)>, BotApiError> {
    match raw.filter(|raw| !raw.is_empty()) {
        None => Ok(None),
        Some(raw) => decode_cursor(raw)
            .map(Some)
            .ok_or_else(|| BotApiError::invalid("This cursor isn't valid.")),
    }
}

/// One message as the API answers it. A deleted message has no text; a flag on a message is not
/// shown, and its text comes as it does to a member.
fn message_json(message: &BotMessageDto, author: Option<&UserDto>) -> Value {
    json!({
        "id": message.id.to_string(),
        "channelId": message.conversation_id.to_string(),
        "threadId": message.thread_id.map(|id| id.to_string()),
        "authorId": message.author_id.to_string(),
        "authorName": author.map(|user| user.display_name.as_ref().unwrap_or(&user.username)),
        "authorIsBot": author.is_some_and(|user| user.is_bot),
        "text": message.text,
        "sentAt": message.sent_at.to_rfc3339(),
        "editedAt": message.edited_at.map(|at| at.to_rfc3339()),
    })
}

/// `GET /me`.
async fn me(req: HttpRequest, api: web::Data<BotApi>) -> Result<HttpResponse, BotApiError> {
    let bot = api.authorize(&req).await?;
    let found = api.bots.bot_of(bot).await;
    let found = api.recorded(bot, COMMAND_ME, json!({}), found).await?;
    Ok(HttpResponse::Ok().json(json!({
        "untrusted": true,
        "id": found.user_id.to_string(),
        "name": found.name,
        "workspaceId": found.server_id.to_string(),
        "runBy": found.run_by,
        "readsGoTo": found.reads_go_to,
    })))
}

/// `GET /channels`: the channels the bot was added to, archived ones left out.
async fn channels(req: HttpRequest, api: web::Data<BotApi>) -> Result<HttpResponse, BotApiError> {
    let bot = api.authorize(&req).await?;
    let mut listed = Vec::new();
    let mut cursor = None;
    let failure = loop {
        let page = api
            .conversations
            .list_channels(ListChannelsQuery {
                actor: Actor::new(bot.user_id),
                server_id: bot.server_id,
                filter: ChannelListFilter {
                    joined_only: true,
                    include_archived: false,
                },
                dir: PageDir::After,
                cursor,
                limit: CHANNELS_PER_PAGE,
            })
            .await;
        match page {
            Ok(page) => {
                listed.extend(page.items.into_iter().map(|channel| {
                    json!({
                        "id": channel.id.to_string(),
                        "name": channel.title,
                        "private": channel.is_private,
                    })
                }));
                match page.next {
                    Some(next) => cursor = Some(next),
                    None => break None,
                }
            }
            Err(err) => break Some(err),
        }
    };
    let outcome = failure.map_or(Ok(()), Err);
    api.recorded(bot, COMMAND_CHANNELS, json!({}), outcome)
        .await?;
    Ok(HttpResponse::Ok().json(json!({ "untrusted": true, "channels": listed })))
}

/// `GET /channels/{id}/messages`: a page of a channel, oldest first, and a `cursor` for the page
/// before it, null when there is none. The read runs on the message service, because the command
/// for recent messages takes no position and answers no ids.
async fn list_messages(
    req: HttpRequest,
    api: web::Data<BotApi>,
    path: web::Path<String>,
) -> Result<HttpResponse, BotApiError> {
    let bot = api.authorize(&req).await?;
    let channel = api.joined_channel(bot, &path).await?;
    let query = call_query(&req)?;
    let limit = page_limit(query.limit.as_deref())?;
    let before = cursor_param(query.before.as_deref())?;
    let page = api
        .messages
        .list_messages(ListMessagesQuery {
            actor: Actor::new(bot.user_id),
            server_id: bot.server_id,
            conversation_id: channel,
            // The list runs newest first, so going further back is `After` in its order.
            dir: PageDir::After,
            cursor: before.map(|(sent_at, id)| MessagePageCursor {
                sent_at,
                message_id: id.into(),
            }),
            limit,
            include_deleted: false,
        })
        .await;
    let args = json!({ "conversation_id": channel.to_string(), "limit": limit });
    let page = api.recorded(bot, COMMAND_RECENT, args, page).await?;
    let cursor = page
        .next
        .map(|next| encode_cursor(next.sent_at, next.message_id.into()));
    let messages = page.items.into_iter().map(BotMessageDto::from).collect();
    Ok(HttpResponse::Ok().json(json!({
        "untrusted": true,
        "messages": api.messages_json(messages).await?,
        "cursor": cursor,
    })))
}

/// `POST /channels/{id}/messages`. The body is read once the token and the channel have passed.
async fn post_message(
    req: HttpRequest,
    api: web::Data<BotApi>,
    path: web::Path<String>,
    payload: web::Payload,
) -> Result<HttpResponse, BotApiError> {
    let bot = api.authorize(&req).await?;
    let channel = api.joined_channel(bot, &path).await?;
    let body = read_bounded(&req, payload, MAX_BODY_BYTES)
        .await
        .map_err(|err| match err {
            BodyError::TooLarge => BotApiError::invalid("The body is over 128 KiB."),
            BodyError::Read => BotApiError::invalid("The body could not be read."),
        })?;
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let text = body["text"]
        .as_str()
        .ok_or_else(|| BotApiError::invalid("The body must be JSON with a string `text`."))?;
    let args = json!({
        "server_id": bot.server_id.to_string(),
        "conversation_id": channel.to_string(),
        "text": text,
    });
    let posted = api.run(bot, COMMAND_POST, args, channel).await?;
    Ok(HttpResponse::Created().json(json!({
        "id": posted["message_id"],
        "channelId": channel.to_string(),
    })))
}

/// `GET /channels/{id}/search`: the command finds the messages in this channel, and the message
/// service gives what the command's answer leaves out.
async fn search(
    req: HttpRequest,
    api: web::Data<BotApi>,
    path: web::Path<String>,
) -> Result<HttpResponse, BotApiError> {
    let bot = api.authorize(&req).await?;
    let channel = api.joined_channel(bot, &path).await?;
    let query = call_query(&req)?;
    let limit = page_limit(query.limit.as_deref())?;
    let text = query.q.as_deref().unwrap_or_default();
    if text.trim().is_empty() || text.chars().count() > MAX_QUERY_CHARS {
        return Err(BotApiError::invalid("q must be 1 to 500 characters."));
    }
    // `in:` answers differently for a channel name that exists and one that does not.
    if parse_search_query(text)?.in_channel.is_some() {
        return Err(BotApiError::invalid(
            "A search is in the channel of its path, so `in:` isn't taken here.",
        ));
    }
    let args = json!({
        "server_id": bot.server_id.to_string(),
        "conversation_id": channel.to_string(),
        "query": text,
        "limit": limit,
    });
    let found = api.run(bot, COMMAND_SEARCH, args, channel).await?;
    let ids: Vec<MessageId> = found["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|hit| hit["id"].as_str())
        .filter_map(|id| Uuid::parse_str(id).ok())
        .map(MessageId::from)
        .collect();
    let mut by_id: HashMap<MessageId, BotMessageDto> = api
        .messages
        .get_messages_by_ids(&ids)
        .await?
        .into_iter()
        .filter(|message| message.conversation_id == channel)
        .map(|message| (message.id, BotMessageDto::from(message)))
        .collect();
    let ordered = ids.iter().filter_map(|id| by_id.remove(id)).collect();
    Ok(HttpResponse::Ok().json(json!({
        "untrusted": true,
        "messages": api.messages_json(ordered).await?,
    })))
}

/// `GET /mentions`: the messages mentioning the bot, oldest first, and the `cursor` of the next
/// call, which is the one given when nothing new came.
async fn mentions(req: HttpRequest, api: web::Data<BotApi>) -> Result<HttpResponse, BotApiError> {
    let bot = api.authorize(&req).await?;
    let query = call_query(&req)?;
    let limit = page_limit(query.limit.as_deref())?;
    let after = cursor_param(query.after.as_deref())?.map(|(created_at, id)| BotMentionCursor {
        created_at,
        message_id: id.into(),
    });
    let page = api.bots.list_mentions(bot, after, limit).await?;
    let cursor = page
        .last
        .or(after)
        .map(|at| encode_cursor(at.created_at, at.message_id.into()));
    Ok(HttpResponse::Ok().json(json!({
        "untrusted": true,
        "mentions": api.messages_json(page.messages).await?,
        "cursor": cursor,
    })))
}

/// A path under the API that is none of its calls: the same `404` as anything else not there.
async fn no_such_call() -> HttpResponse {
    BotApiError::not_found().error_response()
}

/// The routes of the bot API.
pub(crate) fn scope() -> actix_web::Scope {
    web::scope("/api/bot/v1")
        .service(web::resource("/me").route(web::get().to(me)))
        .service(web::resource("/channels").route(web::get().to(channels)))
        .service(
            web::resource("/channels/{id}/messages")
                .route(web::get().to(list_messages))
                .route(web::post().to(post_message)),
        )
        .service(web::resource("/channels/{id}/search").route(web::get().to(search)))
        .service(web::resource("/mentions").route(web::get().to(mentions)))
        .default_service(web::to(no_such_call))
}

#[cfg(test)]
mod tests {
    use actix_web::test as actix_test;
    use actix_web::{App, body};
    use rim_core::bot::model::BotTokenId;
    use rim_services::bot::dto::{BotDto, BotMentionsPage};
    use rim_services::bot::service::MockBotService;
    use rim_services::command::dto::CommandResultDto;
    use rim_services::command::service::MockCommandService;
    use rim_services::conversation::service::MockConversationService;
    use rim_services::message::service::MockMessageService;
    use rim_services::user::service::MockUserService;

    use super::*;

    const TOKEN: &str = "rimbot_abcdefghijklmnopqrstuvwxyz0123456789_-ABCDE";

    /// Mocks a test sets up. A call nothing expected panics, so what a test leaves out shows
    /// which services a refused call never reaches.
    struct Fx {
        bots: MockBotService,
        commands: MockCommandService,
        messages: MockMessageService,
        conversations: MockConversationService,
        users: MockUserService,
        calls_per_window: u32,
    }

    impl Fx {
        fn new() -> Self {
            Self {
                bots: MockBotService::new(),
                commands: MockCommandService::new(),
                messages: MockMessageService::new(),
                conversations: MockConversationService::new(),
                users: MockUserService::new(),
                calls_per_window: TOKEN_CALLS_PER_WINDOW,
            }
        }

        /// The token works, for `bot`.
        fn authenticated(mut self, bot: BotActorDto) -> Self {
            self.bots
                .expect_authenticate_token()
                .returning(move |_| Box::pin(async move { Ok(Some(bot)) }));
            self
        }

        fn in_channel(mut self, joined: bool) -> Self {
            self.bots
                .expect_is_in_channel()
                .returning(move |_, _| Box::pin(async move { Ok(joined) }));
            self
        }

        fn command_answers(mut self, result: ServiceResult<CommandResultDto>) -> Self {
            let mut result = Some(result);
            self.commands.expect_execute_command().returning(move |_| {
                let result = result.take().expect("one call");
                Box::pin(async move { result })
            });
            self
        }

        fn api(self) -> web::Data<BotApi> {
            web::Data::new(BotApi {
                bots: Arc::new(self.bots),
                commands: Arc::new(self.commands),
                messages: Arc::new(self.messages),
                conversations: Arc::new(self.conversations),
                users: Arc::new(self.users),
                token_calls: KeyedRateLimiter::new(TOKEN_WINDOW_SECS, self.calls_per_window),
            })
        }
    }

    fn a_bot() -> BotActorDto {
        BotActorDto {
            user_id: UserId::new(),
            server_id: ServerId::new(),
            token_id: BotTokenId::new(),
        }
    }

    /// The status, `Retry-After` and body of one request to a service built from `fx`.
    async fn send(
        fx: Fx,
        request: actix_test::TestRequest,
    ) -> (StatusCode, Option<String>, String) {
        let app = actix_test::init_service(App::new().app_data(fx.api()).service(scope())).await;
        let response = actix_test::call_service(&app, request.to_request()).await;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(header::RETRY_AFTER)
            .map(|v| v.to_str().expect("header").to_string());
        let bytes = body::to_bytes(response.into_body()).await.expect("body");
        (
            status,
            retry_after,
            String::from_utf8(bytes.to_vec()).expect("text"),
        )
    }

    fn get(path: &str) -> actix_test::TestRequest {
        actix_test::TestRequest::get()
            .uri(&format!("/api/bot/v1{path}"))
            .insert_header((header::AUTHORIZATION, format!("Bearer {TOKEN}")))
    }

    fn post_to(channel: ConversationId, body: &Value) -> actix_test::TestRequest {
        actix_test::TestRequest::post()
            .uri(&format!("/api/bot/v1/channels/{channel}/messages"))
            .insert_header((header::AUTHORIZATION, format!("Bearer {TOKEN}")))
            .set_json(body)
    }

    fn command_ok(payload: Value) -> CommandResultDto {
        CommandResultDto {
            status: CommandResultStatusDto::Success,
            message: "ok".to_string(),
            payload: Some(payload),
            follow_up_commands: vec![],
            pipeline_stages: None,
            confirmation_required: None,
        }
    }

    const BAD_TOKEN: &str =
        r#"{"refusal":"DENIED","message":"This token isn't valid.","retrySafe":false}"#;
    const NOT_FOUND: &str =
        r#"{"refusal":"INVALID","message":"There is nothing here.","retrySafe":false}"#;
    const FAILED: &str =
        r#"{"refusal":"FAILED","message":"RIM could not do this.","retrySafe":false}"#;

    // ── authorize ────────────────────────────────────────────────────────

    #[actix_web::test]
    async fn no_token_and_every_bad_token_give_the_same_bytes() {
        let bare = || actix_test::TestRequest::get().uri("/api/bot/v1/me");
        for request in [
            bare(),
            bare().insert_header((header::AUTHORIZATION, "Basic abc")),
            bare().insert_header((header::AUTHORIZATION, "Bearer")),
            bare().insert_header((header::COOKIE, "rim_session=abc; rim_csrf=def")),
        ] {
            let (status, _, bytes) = send(Fx::new(), request).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(bytes, BAD_TOKEN);
        }

        let mut fx = Fx::new();
        fx.bots
            .expect_authenticate_token()
            .returning(|_| Box::pin(async { Ok(None) }));
        let (status, _, bytes) = send(fx, get("/me")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(bytes, BAD_TOKEN);
    }

    #[actix_web::test]
    async fn a_failed_token_check_is_a_failure_and_shows_nothing_of_it() {
        let mut fx = Fx::new();
        fx.bots.expect_authenticate_token().returning(|_| {
            Box::pin(async { Err(ServiceError::internal(anyhow::anyhow!("db password=x"))) })
        });
        let (status, _, bytes) = send(fx, get("/me")).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(bytes, FAILED);
    }

    #[actix_web::test]
    async fn a_token_over_its_limit_gets_429_and_another_token_does_not() {
        let bot = a_bot();
        let mut fx = Fx::new().authenticated(bot);
        fx.calls_per_window = 1;
        fx.bots.expect_bot_of().returning(move |_| {
            Box::pin(async move {
                Ok(BotDto {
                    user_id: bot.user_id,
                    server_id: bot.server_id,
                    name: "Helper".to_string(),
                    run_by: "Ada".to_string(),
                    reads_go_to: "Nowhere".to_string(),
                    added_by_name: None,
                    created_at: Utc::now(),
                    deactivated_at: None,
                })
            })
        });
        fx.bots
            .expect_record_read()
            .returning(|_, _, _, _| Box::pin(async {}));
        let app = actix_test::init_service(App::new().app_data(fx.api()).service(scope())).await;

        let first = actix_test::call_service(&app, get("/me").to_request()).await;
        assert_eq!(first.status(), StatusCode::OK);

        let second = actix_test::call_service(&app, get("/me").to_request()).await;
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(second.headers().contains_key(header::RETRY_AFTER));
        let body: Value = actix_test::read_body_json(second).await;
        assert_eq!(body["refusal"], "FAILED");
        assert_eq!(body["retrySafe"], true);
    }

    // ── Error mapping ────────────────────────────────────────────────────

    #[test]
    fn each_error_maps_to_its_status_and_kind() {
        let until = Utc::now();
        for (err, status, kind) in [
            (ServiceError::NotFound, StatusCode::NOT_FOUND, "INVALID"),
            (
                ServiceError::Validation("too long".to_string()),
                StatusCode::BAD_REQUEST,
                "INVALID",
            ),
            (
                ServiceError::Conflict("clash".to_string()),
                StatusCode::BAD_REQUEST,
                "INVALID",
            ),
            (
                ServiceError::PermissionDenied,
                StatusCode::FORBIDDEN,
                "DENIED",
            ),
            (
                ServiceError::CapabilityMissing {
                    capability: "chat.write".to_string(),
                },
                StatusCode::FORBIDDEN,
                "DENIED",
            ),
            (
                ServiceError::Forbidden("posting is restricted".to_string()),
                StatusCode::FORBIDDEN,
                "DENIED",
            ),
            (
                ServiceError::TimedOut { until },
                StatusCode::FORBIDDEN,
                "DENIED",
            ),
            (
                ServiceError::ConversationArchived,
                StatusCode::FORBIDDEN,
                "DENIED",
            ),
            (
                ServiceError::ConfirmationStale,
                StatusCode::FORBIDDEN,
                "EXPIRED",
            ),
            (
                ServiceError::RateLimited {
                    retry_after_seconds: 3,
                },
                StatusCode::TOO_MANY_REQUESTS,
                "FAILED",
            ),
            (
                ServiceError::Timeout,
                StatusCode::INTERNAL_SERVER_ERROR,
                "FAILED",
            ),
            (
                ServiceError::upstream("rims1_secret"),
                StatusCode::INTERNAL_SERVER_ERROR,
                "FAILED",
            ),
        ] {
            let mapped = BotApiError::from(err);
            assert_eq!(mapped.status, status, "{mapped:?}");
            assert_eq!(mapped.refusal.as_str(), kind, "{mapped:?}");
        }
    }

    #[test]
    fn nothing_internal_reaches_a_failure_and_a_retry_is_as_the_command_says() {
        for (err, retry_safe) in [
            (ServiceError::internal(anyhow::anyhow!("secret")), false),
            (ServiceError::upstream("rims1_secret"), false),
            (ServiceError::Unavailable, false),
            (
                ServiceError::CommandFailed {
                    failure: Box::new(ServiceError::internal(anyhow::anyhow!("secret"))),
                    retry_safe: true,
                },
                true,
            ),
            (ServiceError::Unauthenticated, false),
        ] {
            let mapped = BotApiError::from(err);
            assert_eq!(mapped.status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(mapped.message, "RIM could not do this.");
            assert_eq!(mapped.retry_safe, retry_safe);
        }
    }

    fn message_dto() -> rim_services::message::dto::MessageDto {
        rim_services::message::dto::MessageDto {
            id: MessageId::new(),
            server_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            thread_id: None,
            root_thread_id: None,
            author_id: UserId::new(),
            text: Some("the text".to_string()),
            has_attachments: false,
            attachments: vec![],
            parent_message_id: None,
            forwarded_from_message_id: None,
            thread_root_message_id: None,
            is_deleted: false,
            sent_at: Utc::now(),
            edited_at: None,
            encryption: None,
            flagged_at: None,
        }
    }

    #[test]
    fn a_deleted_message_has_no_text_or_edit_time() {
        let message = BotMessageDto::from(rim_services::message::dto::MessageDto {
            is_deleted: true,
            edited_at: Some(Utc::now()),
            ..message_dto()
        });
        let shown = message_json(&message, None);
        assert_eq!(shown["text"], Value::Null);
        assert_eq!(shown["editedAt"], Value::Null);
        assert_eq!(shown["authorName"], Value::Null);
        assert_eq!(shown["authorIsBot"], false);
    }

    #[test]
    fn a_flagged_message_comes_as_it_does_to_a_member_and_the_flag_is_not_shown() {
        let message = BotMessageDto::from(rim_services::message::dto::MessageDto {
            flagged_at: Some(Utc::now()),
            ..message_dto()
        });
        let shown = message_json(&message, None);
        assert_eq!(shown["text"], "the text");
        assert!(shown.get("flaggedAt").is_none() && shown.get("flagged").is_none());
    }

    #[test]
    fn a_cursor_round_trips_and_nothing_else_is_one() {
        let at = DateTime::from_timestamp_micros(1_700_000_000_123_456).expect("time");
        let id = Uuid::now_v7();
        assert_eq!(decode_cursor(&encode_cursor(at, id)), Some((at, id)));
        for bad in [
            "",
            "abc",
            "12_",
            "_abc",
            "12_not-a-uuid",
            "x_00000000000000000000000000000000",
        ] {
            assert_eq!(decode_cursor(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_limit_is_from_one_to_a_hundred_with_fifty_when_absent() {
        assert_eq!(page_limit(None).expect("default"), DEFAULT_LIMIT);
        assert_eq!(page_limit(Some("")).expect("empty"), DEFAULT_LIMIT);
        assert_eq!(page_limit(Some("1")).expect("one"), 1);
        assert_eq!(page_limit(Some("100")).expect("most"), 100);
        for bad in ["0", "101", "-1", "many"] {
            assert!(page_limit(Some(bad)).is_err(), "{bad}");
        }
    }

    // ── Calls ────────────────────────────────────────────────────────────

    #[actix_web::test]
    async fn a_channel_not_joined_stops_before_any_command() {
        let channel = ConversationId::new();
        let fx = Fx::new().authenticated(a_bot()).in_channel(false);
        let (status, _, bytes) = send(fx, post_to(channel, &json!({ "text": "x" }))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(bytes, NOT_FOUND);

        // A malformed id never reaches the check.
        let fx = Fx::new().authenticated(a_bot());
        let (status, _, bytes) = send(fx, get("/channels/nope/messages")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(bytes, NOT_FOUND);
    }

    #[actix_web::test]
    async fn a_post_runs_the_fixed_command_as_the_bot_with_nothing_but_its_fields() {
        let bot = a_bot();
        let channel = ConversationId::new();
        let mut fx = Fx::new().authenticated(bot).in_channel(true);
        let message_id = MessageId::new();
        fx.commands
            .expect_execute_command()
            .withf(move |cmd| {
                let context = cmd.context.as_ref().expect("context");
                cmd.command_id == "chat.post_message"
                    && cmd.actor.user_id == bot.user_id
                    && cmd.initiator == CommandInitiator::Bot
                    && !cmd.is_pipe
                    && cmd.confirmation.is_none()
                    && cmd.run_id.is_none()
                    && context.workspace_id == Some(bot.server_id)
                    && context.conversation_id == Some(channel)
                    && cmd.args
                        == Some(json!({
                            "server_id": bot.server_id.to_string(),
                            "conversation_id": channel.to_string(),
                            "text": "hello",
                        }))
            })
            .returning(move |_| {
                Box::pin(async move {
                    Ok(command_ok(json!({
                        "message_id": message_id.to_string(),
                        "conversation_id": channel.to_string(),
                    })))
                })
            });
        let body = json!({ "text": "hello", "command_id": "admin.kick", "server_id": "x" });
        let (status, _, bytes) = send(fx, post_to(channel, &body)).await;
        assert_eq!(status, StatusCode::CREATED, "{bytes}");
        let answer: Value = serde_json::from_str(&bytes).expect("json");
        assert_eq!(
            answer,
            json!({ "id": message_id.to_string(), "channelId": channel.to_string() })
        );
    }

    #[actix_web::test]
    async fn a_command_asking_for_a_confirmation_is_denied() {
        let fx = Fx::new()
            .authenticated(a_bot())
            .in_channel(true)
            .command_answers(Ok(CommandResultDto {
                status: CommandResultStatusDto::ConfirmationRequired,
                ..command_ok(Value::Null)
            }));
        let (status, _, bytes) =
            send(fx, post_to(ConversationId::new(), &json!({ "text": "x" }))).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let answer: Value = serde_json::from_str(&bytes).expect("json");
        assert_eq!(answer["refusal"], "DENIED");
    }

    #[actix_web::test]
    async fn a_command_that_ended_in_an_error_shows_nothing_of_it() {
        let fx = Fx::new()
            .authenticated(a_bot())
            .in_channel(true)
            .command_answers(Ok(CommandResultDto {
                status: CommandResultStatusDto::Error,
                message: "pipe stage 1 failed: secret".to_string(),
                ..command_ok(Value::Null)
            }));
        let (status, _, bytes) =
            send(fx, post_to(ConversationId::new(), &json!({ "text": "x" }))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(bytes, FAILED);
    }

    #[actix_web::test]
    async fn a_refused_command_keeps_its_retry_after() {
        let fx = Fx::new()
            .authenticated(a_bot())
            .in_channel(true)
            .command_answers(Err(ServiceError::RateLimited {
                retry_after_seconds: 42,
            }));
        let (status, retry_after, bytes) =
            send(fx, post_to(ConversationId::new(), &json!({ "text": "x" }))).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(retry_after.as_deref(), Some("42"));
        let answer: Value = serde_json::from_str(&bytes).expect("json");
        assert_eq!(answer["retrySafe"], true);
    }

    #[actix_web::test]
    async fn an_empty_page_of_mentions_gives_the_cursor_back() {
        let mut fx = Fx::new().authenticated(a_bot());
        fx.bots.expect_list_mentions().returning(|_, _, _| {
            Box::pin(async {
                Ok(BotMentionsPage {
                    messages: vec![],
                    last: None,
                })
            })
        });
        let at = DateTime::from_timestamp_micros(1_700_000_000_000_000).expect("time");
        let cursor = encode_cursor(at, Uuid::now_v7());
        let (status, _, bytes) = send(fx, get(&format!("/mentions?after={cursor}"))).await;
        assert_eq!(status, StatusCode::OK);
        let answer: Value = serde_json::from_str(&bytes).expect("json");
        assert_eq!(answer["cursor"], json!(cursor));
        assert_eq!(answer["untrusted"], true);
    }
}
