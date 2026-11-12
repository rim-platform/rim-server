//! Incoming webhooks: `POST /api/hooks/{token}`, which posts to one channel as one bot. The same
//! route takes deliveries to a test session's URL, which post nothing and are held in memory, and
//! hands a copy of each webhook delivery to the test sessions listening on that webhook.
//!
//! The token is in the path, so nothing here may let it out: it is read from the match info, not
//! through the path extractor, whose failure logs the request's path, and it goes into no span,
//! log line, metric label or error. No cookie is read. A post runs `chat.post_message` as the bot,
//! so a moderation action on the bot stops it as it stops the bot.

use std::sync::Arc;

use actix_web::http::{StatusCode, header};
use actix_web::{HttpRequest, HttpResponse, web};
use chrono::Utc;
use rim_core::bot::model::WebhookTestSessionId;
use rim_infra_services::KeyedRateLimiter;
use rim_services::bot::dto::delivery_headers;
use rim_services::bot::service::BotService;
use rim_services::bot::test_session::MAX_TEST_BODY_BYTES;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::command::refusal::Refusal;
use rim_services::command::service::CommandService;
use rim_services::provider::AppServiceProvider;
use serde_json::json;

use crate::body::{BodyError, read_bounded};
use crate::bot_api::{BotApiError, COMMAND_POST, run_command};

/// The window of a webhook's post limit, in seconds.
const WINDOW_SECS: u64 = 60;
/// The posts one webhook may make in [`WINDOW_SECS`].
const POSTS_PER_WINDOW: u32 = 30;
/// The largest request body, in bytes.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// The answer to a post that ran.
const POSTED: &str = r#"{"ok":true}"#;
/// The answer to a body with nothing to post.
const IGNORED: &str = r#"{"ok":true,"ignored":true}"#;
/// The answer to every token that does not work, the same bytes whatever the cause.
const BAD_TOKEN: &str = r#"{"ok":false}"#;
/// The answer to a delivery a test session held.
const TESTED: &str = r#"{"ok":true,"test":true}"#;

/// What the webhook route uses, shared by the server's workers.
pub(crate) struct WebhookApi {
    bots: Arc<dyn BotService>,
    commands: Arc<dyn CommandService>,
    /// Each webhook's posts per minute, charged after its token is known to work, and each test
    /// session's deliveries per minute, under a key of their own.
    posts: KeyedRateLimiter,
}

impl WebhookApi {
    pub(crate) fn new(provider: &dyn AppServiceProvider) -> Self {
        Self {
            bots: provider.bot_service(),
            commands: provider.command_service(),
            posts: KeyedRateLimiter::new(WINDOW_SECS, POSTS_PER_WINDOW),
        }
    }
}

fn answer(status: StatusCode, body: &'static str) -> HttpResponse {
    HttpResponse::build(status)
        .content_type(header::ContentType::json())
        .body(body)
}

/// The headers of `req` a format reads: the `x-` ones of plain text, as
/// [`delivery_headers`] keeps them.
fn format_headers(req: &HttpRequest) -> Vec<(String, String)> {
    delivery_headers(
        req.headers()
            .iter()
            .filter_map(|(name, value)| Some((name.as_str(), value.to_str().ok()?))),
    )
}

/// `req`'s `Content-Type`, when it is plain text.
fn content_type(req: &HttpRequest) -> Option<String> {
    req.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// A delivery to test session `session`'s URL: charged against its own limit, held, and answered
/// without posting anything.
async fn test_delivery(
    req: &HttpRequest,
    api: &WebhookApi,
    session: WebhookTestSessionId,
    payload: web::Payload,
) -> Result<HttpResponse, BotApiError> {
    if let RateLimitDecision::Limited {
        retry_after_seconds,
    } = api
        .posts
        .check_and_record(&format!("test:{session}"), Utc::now())
    {
        rim_infra_services::metrics::rate_limited("webhook");
        return Err(BotApiError::rate_limited(retry_after_seconds));
    }
    let headers = format_headers(req);
    match read_bounded(req, payload, MAX_TEST_BODY_BYTES).await {
        Ok(body) => {
            api.bots
                .record_test_delivery(session, content_type(req), &headers, &body, false);
            Ok(answer(StatusCode::OK, TESTED))
        }
        Err(BodyError::TooLarge) => {
            api.bots
                .record_test_delivery(session, content_type(req), &headers, &[], true);
            Err(BotApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                Refusal::Invalid,
                "The body is over 256 KiB.",
                false,
            ))
        }
        Err(BodyError::Read) => Err(BotApiError::invalid("The body could not be read.")),
    }
}

/// `POST /api/hooks/{token}`. The limit is charged and the body read only once the token has
/// passed, so a request without a working token costs no more than the lookup.
async fn post(
    req: HttpRequest,
    api: web::Data<WebhookApi>,
    payload: web::Payload,
) -> Result<HttpResponse, BotApiError> {
    let token = req.match_info().get("token").unwrap_or_default();
    let Some(webhook) = api.bots.resolve_webhook(token).await? else {
        return match api.bots.resolve_test_session(token) {
            Some(session) => test_delivery(&req, &api, session, payload).await,
            None => Ok(answer(StatusCode::UNAUTHORIZED, BAD_TOKEN)),
        };
    };
    if let RateLimitDecision::Limited {
        retry_after_seconds,
    } = api
        .posts
        .check_and_record(&webhook.id.to_string(), Utc::now())
    {
        rim_infra_services::metrics::rate_limited("webhook");
        return Err(BotApiError::rate_limited(retry_after_seconds));
    }
    let headers = format_headers(&req);
    let body = match read_bounded(&req, payload, MAX_BODY_BYTES).await {
        Ok(body) => body,
        Err(BodyError::TooLarge) => {
            api.bots
                .copy_to_test_sessions(webhook.id, content_type(&req), &headers, &[], true);
            return Err(BotApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                Refusal::Invalid,
                "The body is over 1 MiB.",
                false,
            ));
        }
        Err(BodyError::Read) => return Err(BotApiError::invalid("The body could not be read.")),
    };
    api.bots
        .copy_to_test_sessions(webhook.id, content_type(&req), &headers, &body, false);
    let Some(text) = api.bots.webhook_text(&webhook, &headers, &body)? else {
        return Ok(answer(StatusCode::ACCEPTED, IGNORED));
    };
    let args = json!({
        "server_id": webhook.server_id.to_string(),
        "conversation_id": webhook.conversation_id.to_string(),
        "text": text,
    });
    run_command(
        api.commands.as_ref(),
        webhook.bot_user_id,
        webhook.server_id,
        COMMAND_POST,
        args,
        webhook.conversation_id,
    )
    .await?;
    Ok(answer(StatusCode::OK, POSTED))
}

/// The webhook route.
pub(crate) fn resource() -> actix_web::Resource {
    web::resource("/api/hooks/{token}").route(web::post().to(post))
}

#[cfg(test)]
mod tests {
    use actix_web::test as actix_test;
    use actix_web::{App, body};
    use rim_core::bot::model::{IncomingWebhookId, WebhookPreset};
    use rim_core::conversation::model::ConversationId;
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::ServiceError;
    use rim_services::bot::dto::{IncomingWebhookDto, MAX_HEADER_VALUE_CHARS};
    use rim_services::bot::service::MockBotService;
    use rim_services::bot::test_session::MAX_TEST_BODY_BYTES;
    use rim_services::command::dto::{CommandResultDto, CommandResultStatusDto};
    use rim_services::command::initiator::CommandInitiator;
    use rim_services::command::service::MockCommandService;
    use serde_json::Value;

    use super::*;

    const TOKEN: &str = "rimhook_abcdefghijklmnopqrstuvwxyz0123456789_-ABCDE";

    struct Fx {
        bots: MockBotService,
        commands: MockCommandService,
        posts_per_window: u32,
        /// Whether no test session exists, so no token is one's and copies go nowhere.
        no_sessions: bool,
    }

    impl Fx {
        fn new() -> Self {
            Self {
                bots: MockBotService::new(),
                commands: MockCommandService::new(),
                posts_per_window: POSTS_PER_WINDOW,
                no_sessions: true,
            }
        }

        fn resolving(mut self, webhook: IncomingWebhookDto) -> Self {
            self.bots.expect_resolve_webhook().returning(move |_| {
                let webhook = webhook.clone();
                Box::pin(async move { Ok(Some(webhook)) })
            });
            self
        }

        /// The preset's text is `text`.
        fn with_text(mut self, text: Option<&'static str>) -> Self {
            self.bots
                .expect_webhook_text()
                .returning(move |_, _, _| Ok(text.map(str::to_string)));
            self
        }

        fn api(mut self) -> web::Data<WebhookApi> {
            if self.no_sessions {
                self.bots.expect_resolve_test_session().returning(|_| None);
                self.bots
                    .expect_copy_to_test_sessions()
                    .returning(|_, _, _, _, _| ());
            }
            web::Data::new(WebhookApi {
                bots: Arc::new(self.bots),
                commands: Arc::new(self.commands),
                posts: KeyedRateLimiter::new(WINDOW_SECS, self.posts_per_window),
            })
        }
    }

    fn a_webhook(preset: WebhookPreset) -> IncomingWebhookDto {
        IncomingWebhookDto {
            id: IncomingWebhookId::new(),
            server_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            bot_user_id: UserId::new(),
            preset,
            template: None,
            created_at: Utc::now(),
            last_used_at: None,
            revoked_at: None,
        }
    }

    fn posted() -> CommandResultDto {
        CommandResultDto {
            status: CommandResultStatusDto::Success,
            message: "ok".to_string(),
            payload: Some(json!({ "message_id": "x" })),
            follow_up_commands: vec![],
            pipeline_stages: None,
            confirmation_required: None,
        }
    }

    fn request() -> actix_test::TestRequest {
        actix_test::TestRequest::post().uri(&format!("/api/hooks/{TOKEN}"))
    }

    async fn send(fx: Fx, request: actix_test::TestRequest) -> (StatusCode, String) {
        let app = actix_test::init_service(App::new().app_data(fx.api()).service(resource())).await;
        let response = actix_test::call_service(&app, request.to_request()).await;
        let status = response.status();
        let bytes = body::to_bytes(response.into_body()).await.expect("body");
        (status, String::from_utf8(bytes.to_vec()).expect("text"))
    }

    #[actix_web::test]
    async fn a_token_that_does_not_work_is_the_same_bytes_and_reaches_nothing() {
        let mut fx = Fx::new();
        fx.bots
            .expect_resolve_webhook()
            .withf(|raw| raw == TOKEN)
            .returning(|_| Box::pin(async { Ok(None) }));
        let (status, bytes) = send(fx, request().set_payload("not even json")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(bytes, r#"{"ok":false}"#);
    }

    #[actix_web::test]
    async fn a_session_cookie_means_nothing() {
        let mut fx = Fx::new();
        fx.bots
            .expect_resolve_webhook()
            .withf(|raw| raw == "nope")
            .returning(|_| Box::pin(async { Ok(None) }));
        let request = actix_test::TestRequest::post()
            .uri("/api/hooks/nope")
            .insert_header((header::COOKIE, "rim_session=abc; rim_csrf=def"))
            .insert_header(("x-csrf-token", "def"));
        let (status, bytes) = send(fx, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(bytes, r#"{"ok":false}"#);
    }

    #[actix_web::test]
    async fn a_failed_lookup_shows_nothing_of_it() {
        let mut fx = Fx::new();
        fx.bots.expect_resolve_webhook().returning(|_| {
            Box::pin(async { Err(ServiceError::internal(anyhow::anyhow!("db password=x"))) })
        });
        let (status, bytes) = send(fx, request()).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!bytes.contains("password"), "{bytes}");
    }

    #[actix_web::test]
    async fn a_post_runs_the_fixed_command_as_the_bot() {
        let webhook = a_webhook(WebhookPreset::Plain);
        let expected = webhook.clone();
        let mut fx = Fx::new().resolving(webhook).with_text(Some("hello"));
        fx.commands
            .expect_execute_command()
            .withf(move |cmd| {
                let context = cmd.context.as_ref().expect("context");
                cmd.command_id == "chat.post_message"
                    && cmd.actor.user_id == expected.bot_user_id
                    && cmd.initiator == CommandInitiator::Bot
                    && cmd.confirmation.is_none()
                    && context.conversation_id == Some(expected.conversation_id)
                    && context.workspace_id == Some(expected.server_id)
                    && cmd.args
                        == Some(json!({
                            "server_id": expected.server_id.to_string(),
                            "conversation_id": expected.conversation_id.to_string(),
                            "text": "hello",
                        }))
            })
            .times(1)
            .returning(|_| Box::pin(async { Ok(posted()) }));
        let (status, bytes) = send(fx, request().set_payload("{}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bytes, r#"{"ok":true}"#);
    }

    #[actix_web::test]
    async fn nothing_to_post_is_202_and_runs_no_command() {
        let fx = Fx::new()
            .resolving(a_webhook(WebhookPreset::Github))
            .with_text(None);
        let (status, bytes) = send(fx, request().set_payload("{}")).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(bytes, r#"{"ok":true,"ignored":true}"#);
    }

    #[actix_web::test]
    async fn an_unreadable_body_is_400_and_runs_no_command() {
        let mut fx = Fx::new().resolving(a_webhook(WebhookPreset::Plain));
        fx.bots.expect_webhook_text().returning(|_, _, _| {
            Err(ServiceError::Validation(
                "The body must be JSON.".to_string(),
            ))
        });
        let (status, bytes) = send(fx, request().set_payload("nope")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let answer: Value = serde_json::from_str(&bytes).expect("json");
        assert_eq!(answer["refusal"], "INVALID");
    }

    #[actix_web::test]
    async fn the_event_header_reaches_the_format_capped_and_no_other_header_does() {
        let event = |value: String| vec![("x-github-event".to_string(), value)];
        for (preset, sent, expected) in [
            (
                WebhookPreset::Github,
                Some("issues".to_string()),
                event("issues".to_string()),
            ),
            (
                WebhookPreset::Plain,
                Some("issues".to_string()),
                event("issues".to_string()),
            ),
            (WebhookPreset::Github, None, vec![]),
            (
                WebhookPreset::Github,
                Some("e".repeat(MAX_HEADER_VALUE_CHARS)),
                event("e".repeat(MAX_HEADER_VALUE_CHARS)),
            ),
            (
                WebhookPreset::Github,
                Some("e".repeat(MAX_HEADER_VALUE_CHARS + 1)),
                vec![],
            ),
        ] {
            let mut fx = Fx::new().resolving(a_webhook(preset));
            let webhook_preset = preset;
            fx.bots
                .expect_webhook_text()
                .withf(move |webhook, headers, _| {
                    webhook.preset == webhook_preset && headers == expected.as_slice()
                })
                .times(1)
                .returning(|_, _, _| Ok(None));
            let mut request = request()
                .set_payload("{}")
                .insert_header((header::AUTHORIZATION, "Bearer secret"))
                .insert_header((header::COOKIE, "rim_session=abc"));
            if let Some(sent) = sent {
                request = request.insert_header(("X-GitHub-Event", sent));
            }
            let (status, _) = send(fx, request).await;
            assert_eq!(status, StatusCode::ACCEPTED);
        }
    }

    #[actix_web::test]
    async fn over_the_limit_is_429_before_the_body_is_read_and_another_webhook_is_not_held() {
        let first = a_webhook(WebhookPreset::Plain);
        let second = a_webhook(WebhookPreset::Plain);
        let mut fx = Fx::new().with_text(None);
        fx.posts_per_window = 1;
        let webhooks = (first, second);
        fx.bots.expect_resolve_webhook().returning(move |raw| {
            let webhook = if raw == TOKEN {
                webhooks.0.clone()
            } else {
                webhooks.1.clone()
            };
            Box::pin(async move { Ok(Some(webhook)) })
        });
        let app = actix_test::init_service(App::new().app_data(fx.api()).service(resource())).await;

        let accepted = actix_test::call_service(&app, request().to_request()).await;
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);

        let limited = actix_test::call_service(&app, request().to_request()).await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(limited.headers().contains_key(header::RETRY_AFTER));
        let body: Value = actix_test::read_body_json(limited).await;
        assert_eq!(body["refusal"], "FAILED");
        assert_eq!(body["retrySafe"], true);

        let other = actix_test::call_service(
            &app,
            actix_test::TestRequest::post()
                .uri("/api/hooks/other")
                .to_request(),
        )
        .await;
        assert_eq!(other.status(), StatusCode::ACCEPTED);
    }

    #[actix_web::test]
    async fn a_body_over_the_limit_is_413_and_one_at_it_is_read() {
        let fx = Fx::new().resolving(a_webhook(WebhookPreset::Plain));
        let (status, _) = send(fx, request().set_payload(vec![b' '; MAX_BODY_BYTES + 1])).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

        let fx = Fx::new()
            .resolving(a_webhook(WebhookPreset::Plain))
            .with_text(None);
        let (status, _) = send(fx, request().set_payload(vec![b' '; MAX_BODY_BYTES])).await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }

    #[actix_web::test]
    async fn a_refused_command_answers_in_the_shape_of_the_bot_api() {
        let mut fx = Fx::new()
            .resolving(a_webhook(WebhookPreset::Plain))
            .with_text(Some("x"));
        fx.commands
            .expect_execute_command()
            .returning(|_| Box::pin(async { Err(ServiceError::ConversationArchived) }));
        let (status, bytes) = send(fx, request().set_payload("{}")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let answer: Value = serde_json::from_str(&bytes).expect("json");
        assert_eq!(answer["refusal"], "DENIED");
        assert_eq!(answer["retrySafe"], false);
    }

    #[actix_web::test]
    async fn only_post_is_a_route() {
        let app =
            actix_test::init_service(App::new().app_data(Fx::new().api()).service(resource()))
                .await;
        let response = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri(&format!("/api/hooks/{TOKEN}"))
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    // ── Test sessions ────────────────────────────────────────────────────

    /// A fixture where `TOKEN` is no webhook's but test session `session`'s.
    fn for_session(session: WebhookTestSessionId) -> Fx {
        let mut fx = Fx::new();
        fx.no_sessions = false;
        fx.bots
            .expect_resolve_webhook()
            .returning(|_| Box::pin(async { Ok(None) }));
        fx.bots
            .expect_resolve_test_session()
            .withf(|raw| raw == TOKEN)
            .returning(move |_| Some(session));
        fx
    }

    #[actix_web::test]
    async fn a_delivery_to_a_test_url_is_held_with_its_x_headers_and_posts_nothing() {
        let session = WebhookTestSessionId::new();
        let mut fx = for_session(session);
        fx.bots
            .expect_record_test_delivery()
            .withf(move |id, content_type, headers, body, too_large| {
                *id == session
                    && content_type.as_deref() == Some("application/json")
                    && headers == [("x-github-event".to_string(), "ping".to_string())]
                    && body == br#"{"zen":"hi"}"#
                    && !too_large
            })
            .times(1)
            .returning(|_, _, _, _, _| ());
        let request = request()
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .insert_header(("X-GitHub-Event", "ping"))
            .insert_header((header::AUTHORIZATION, "Bearer secret"))
            .insert_header((header::COOKIE, "rim_session=abc"))
            .set_payload(r#"{"zen":"hi"}"#);
        let (status, bytes) = send(fx, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bytes, r#"{"ok":true,"test":true}"#);
    }

    #[actix_web::test]
    async fn a_test_url_holds_a_body_of_256_kib_and_refuses_one_past_it() {
        let session = WebhookTestSessionId::new();
        let mut fx = for_session(session);
        fx.bots
            .expect_record_test_delivery()
            .withf(|_, _, _, body, too_large| body.len() == MAX_TEST_BODY_BYTES && !too_large)
            .times(1)
            .returning(|_, _, _, _, _| ());
        let (status, _) = send(fx, request().set_payload(vec![b' '; MAX_TEST_BODY_BYTES])).await;
        assert_eq!(status, StatusCode::OK);

        let mut fx = for_session(session);
        fx.bots
            .expect_record_test_delivery()
            .withf(|_, _, _, body, too_large| body.is_empty() && *too_large)
            .times(1)
            .returning(|_, _, _, _, _| ());
        let (status, bytes) = send(
            fx,
            request().set_payload(vec![b' '; MAX_TEST_BODY_BYTES + 1]),
        )
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(bytes.contains("256 KiB"), "{bytes}");
    }

    #[actix_web::test]
    async fn a_test_url_takes_thirty_deliveries_a_minute() {
        let session = WebhookTestSessionId::new();
        let mut fx = for_session(session);
        fx.bots
            .expect_record_test_delivery()
            .times(30)
            .returning(|_, _, _, _, _| ());
        let app = actix_test::init_service(App::new().app_data(fx.api()).service(resource())).await;
        for _ in 0..30 {
            let held = actix_test::call_service(&app, request().to_request()).await;
            assert_eq!(held.status(), StatusCode::OK);
        }
        let limited = actix_test::call_service(&app, request().to_request()).await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(limited.headers().contains_key(header::RETRY_AFTER));
    }

    #[actix_web::test]
    async fn a_webhook_delivery_is_copied_to_its_sessions_and_still_posts() {
        let webhook = a_webhook(WebhookPreset::Plain);
        let webhook_id = webhook.id;
        let mut fx = Fx::new().resolving(webhook).with_text(Some("hello"));
        fx.no_sessions = false;
        fx.bots
            .expect_copy_to_test_sessions()
            .withf(move |id, _, headers, body, too_large| {
                *id == webhook_id
                    && headers == [("x-event".to_string(), "deploy".to_string())]
                    && body == br#"{"text":"hello"}"#
                    && !too_large
            })
            .times(1)
            .returning(|_, _, _, _, _| ());
        fx.commands
            .expect_execute_command()
            .times(1)
            .returning(|_| Box::pin(async { Ok(posted()) }));
        let request = request()
            .insert_header(("x-event", "deploy"))
            .set_payload(r#"{"text":"hello"}"#);
        let (status, bytes) = send(fx, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bytes, r#"{"ok":true}"#);
    }

    #[actix_web::test]
    async fn a_webhook_delivery_past_its_limit_is_copied_as_too_large() {
        let mut fx = Fx::new().resolving(a_webhook(WebhookPreset::Plain));
        fx.no_sessions = false;
        fx.bots
            .expect_copy_to_test_sessions()
            .withf(|_, _, _, body, too_large| body.is_empty() && *too_large)
            .times(1)
            .returning(|_, _, _, _, _| ());
        let (status, _) = send(fx, request().set_payload(vec![b' '; MAX_BODY_BYTES + 1])).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }
}
