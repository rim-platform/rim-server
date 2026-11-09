use serde_json::Value;

use rim_core::notification::model::{DndPause, NotificationLevel};
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};
use rim_services::notification::command::{
    GetNotificationSettingsQuery, MarkNotificationsReadCmd, MarkNotificationsReadTarget,
    SetConversationNotificationLevelCmd, SetDndPauseCmd,
};
use rim_services::notification_preference::command::{MuteConversationCmd, UnmuteConversationCmd};
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::command::CommandServices;

use super::{
    optional_integer_arg, optional_string_arg, require_string_arg, resolve_conversation_id,
    resolve_workspace_id,
};

/// Longest `notifications.dnd` pause given in minutes (7 days).
const MAX_DND_MINUTES: i64 = 7 * 24 * 60;

fn success(message: &str, payload: Value) -> CommandResultDto {
    CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: message.to_string(),
        payload: Some(payload),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    }
}

/// `level` arg: `all`, `mentions`, `none`, or `default` (clears the override).
fn parse_level(raw: &str) -> ServiceResult<Option<NotificationLevel>> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "default" => Ok(None),
        other => other.parse::<NotificationLevel>().map(Some).map_err(|_| {
            ServiceError::Validation(
                "`level` must be one of all, mentions, none, default".to_string(),
            )
        }),
    }
}

pub(super) async fn set_level(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id(&args, context.as_ref(), "server_id")?;
    let conversation_id = resolve_conversation_id(&args, context.as_ref(), "conversation_id")?;
    let level = parse_level(&require_string_arg(&args, "level")?)?;
    let dto = services
        .notification_service
        .set_conversation_level(SetConversationNotificationLevelCmd {
            actor,
            server_id,
            conversation_id,
            level,
        })
        .await?;
    Ok(success(
        "notification level updated",
        serde_json::json!({
            "conversation_id": dto.conversation_id.to_string(),
            "level": dto.level.map(|l| l.as_str()),
            "effective_level": dto.effective_level.as_str(),
        }),
    ))
}

/// `mode` = `on` | `off` | `until` (with `until` RFC 3339 or `minutes`).
fn parse_pause(
    args: &serde_json::Map<String, Value>,
    now: chrono::DateTime<chrono::Utc>,
) -> ServiceResult<DndPause> {
    let mode = require_string_arg(args, "mode")?;
    match mode.trim().to_ascii_lowercase().as_str() {
        "on" => Ok(DndPause::Indefinite),
        "off" => Ok(DndPause::Off),
        "until" => {
            if let Some(raw) = optional_string_arg(args, "until").filter(|s| !s.trim().is_empty()) {
                let until = chrono::DateTime::parse_from_rfc3339(raw.trim())
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .map_err(|_| {
                        ServiceError::Validation(
                            "invalid RFC 3339 timestamp arg `until`".to_string(),
                        )
                    })?;
                return Ok(DndPause::Until(until));
            }
            match optional_integer_arg(args, "minutes") {
                Some(minutes) if (1..=MAX_DND_MINUTES).contains(&minutes) => {
                    Ok(DndPause::Until(now + chrono::Duration::minutes(minutes)))
                }
                Some(_) => Err(ServiceError::Validation(format!(
                    "`minutes` must be between 1 and {MAX_DND_MINUTES}"
                ))),
                None => Err(ServiceError::Validation(
                    "mode `until` needs an `until` time or `minutes`".to_string(),
                )),
            }
        }
        _ => Err(ServiceError::Validation(
            "`mode` must be one of on, off, until".to_string(),
        )),
    }
}

pub(super) async fn dnd(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id(&args, context.as_ref(), "server_id")?;
    let pause = parse_pause(&args, chrono::Utc::now())?;
    let settings = services
        .notification_service
        .set_dnd_pause(SetDndPauseCmd {
            actor,
            server_id,
            pause,
        })
        .await?;
    let (message, until) = match settings.dnd_pause {
        DndPause::Off => ("do not disturb off", None),
        DndPause::Indefinite => ("do not disturb on", None),
        DndPause::Until(until) => ("do not disturb on", Some(until.to_rfc3339())),
    };
    Ok(success(
        message,
        serde_json::json!({
            "paused": settings.dnd_pause != DndPause::Off,
            "until": until,
        }),
    ))
}

/// When a `notifications.mute_conversation` mute ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MuteEnd {
    /// At this time.
    At(chrono::DateTime<chrono::Utc>),
    /// At 09:00 tomorrow in the caller's timezone.
    TomorrowMorning,
    /// Never: the mute lasts until the conversation is unmuted.
    Never,
}

/// `duration` = `1h` | `8h` | `until_tomorrow` | `forever` | `until` (with `until` RFC 3339, in
/// the future). `until` is refused with any other duration.
fn parse_mute_end(
    args: &serde_json::Map<String, Value>,
    now: chrono::DateTime<chrono::Utc>,
) -> ServiceResult<MuteEnd> {
    let duration = require_string_arg(args, "duration")?
        .trim()
        .to_ascii_lowercase();
    let until = optional_string_arg(args, "until").filter(|s| !s.trim().is_empty());
    if until.is_some() && duration != "until" {
        return Err(ServiceError::Validation(
            "`until` is only read with duration `until`".to_string(),
        ));
    }
    match duration.as_str() {
        "1h" => Ok(MuteEnd::At(now + chrono::Duration::hours(1))),
        "8h" => Ok(MuteEnd::At(now + chrono::Duration::hours(8))),
        "until_tomorrow" => Ok(MuteEnd::TomorrowMorning),
        "forever" => Ok(MuteEnd::Never),
        "until" => {
            let raw = until.ok_or_else(|| {
                ServiceError::Validation("duration `until` needs an `until` time".to_string())
            })?;
            let until = chrono::DateTime::parse_from_rfc3339(raw.trim())
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .map_err(|_| {
                    ServiceError::Validation("invalid RFC 3339 timestamp arg `until`".to_string())
                })?;
            if until <= now {
                return Err(ServiceError::Validation(
                    "`until` must be in the future".to_string(),
                ));
            }
            Ok(MuteEnd::At(until))
        }
        _ => Err(ServiceError::Validation(
            "`duration` must be one of 1h, 8h, until_tomorrow, forever, until".to_string(),
        )),
    }
}

/// 09:00 on the day after `now` in `timezone`. A 09:00 the zone skips that day is read an hour
/// later; it never happens in a zone in use today.
fn tomorrow_morning(
    now: chrono::DateTime<chrono::Utc>,
    timezone: chrono_tz::Tz,
) -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;

    let today = now.with_timezone(&timezone).date_naive();
    let tomorrow = today.succ_opt().unwrap_or(today);
    let nine = chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap_or_default();
    let local = tomorrow.and_time(nine);
    timezone
        .from_local_datetime(&local)
        .earliest()
        .or_else(|| {
            timezone
                .from_local_datetime(&(local + chrono::Duration::hours(1)))
                .earliest()
        })
        .map(|at| at.with_timezone(&chrono::Utc))
        .unwrap_or_else(|| chrono::Utc.from_utc_datetime(&local))
}

/// The caller's timezone as the server knows it: that of their daily do-not-disturb window in
/// the workspace, or UTC when they have none.
async fn caller_timezone(
    services: &CommandServices,
    actor: Actor,
    server_id: rim_core::server::model::ServerId,
) -> ServiceResult<chrono_tz::Tz> {
    let settings = services
        .notification_service
        .get_settings(GetNotificationSettingsQuery { actor, server_id })
        .await?;
    Ok(settings
        .dnd
        .and_then(|window| window.timezone.parse::<chrono_tz::Tz>().ok())
        .unwrap_or(chrono_tz::UTC))
}

pub(super) async fn mute_conversation(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id(&args, context.as_ref(), "server_id")?;
    let conversation_id = resolve_conversation_id(&args, context.as_ref(), "conversation_id")?;
    let now = chrono::Utc::now();
    let muted_until = match parse_mute_end(&args, now)? {
        MuteEnd::At(until) => Some(until),
        MuteEnd::TomorrowMorning => Some(tomorrow_morning(
            now,
            caller_timezone(services, actor, server_id).await?,
        )),
        MuteEnd::Never => None,
    };
    services
        .notification_preference_service
        .mute_conversation(MuteConversationCmd {
            actor,
            server_id,
            conversation_id,
            muted_until,
        })
        .await?;
    Ok(success(
        "conversation muted",
        serde_json::json!({
            "conversation_id": conversation_id.to_string(),
            "muted_until": muted_until.map(|until| until.to_rfc3339()),
        }),
    ))
}

pub(super) async fn unmute_conversation(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id(&args, context.as_ref(), "server_id")?;
    let conversation_id = resolve_conversation_id(&args, context.as_ref(), "conversation_id")?;
    services
        .notification_preference_service
        .unmute_conversation(UnmuteConversationCmd {
            actor,
            server_id,
            conversation_id,
        })
        .await?;
    Ok(success(
        "conversation unmuted",
        serde_json::json!({ "conversation_id": conversation_id.to_string() }),
    ))
}

pub(super) async fn mark_all_read(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id(&args, context.as_ref(), "server_id")?;
    let marked = services
        .notification_service
        .mark_read(MarkNotificationsReadCmd {
            actor,
            server_id,
            target: MarkNotificationsReadTarget::AllBefore(chrono::Utc::now()),
        })
        .await?;
    Ok(success(
        "notifications marked read",
        serde_json::json!({ "marked": marked }),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::model::ConversationId;
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::notification::dto::{
        ConversationNotificationLevelDto, NotificationSettingsDto,
    };
    use rim_services::notification::service::MockNotificationService;
    use rim_services::notification_preference::service::MockNotificationPreferenceService;

    use super::*;

    fn services(notification: MockNotificationService) -> CommandServices {
        services_with(notification, MockNotificationPreferenceService::new())
    }

    fn services_with(
        notification: MockNotificationService,
        preferences: MockNotificationPreferenceService,
    ) -> CommandServices {
        CommandServices {
            auth_service: Arc::new(rim_services::auth::service::MockAuthService::new()),
            capability_service: Arc::new(
                rim_services::capability::service::MockCapabilityService::new(),
            ),
            conversation_service: Arc::new(
                rim_services::conversation::service::MockConversationService::new(),
            ),
            message_service: Arc::new(rim_services::message::service::MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(rim_services::pin::service::MockPinService::new()),
            read_state_service: Arc::new(
                rim_services::read_state::service::MockReadStateService::new(),
            ),
            server_member_service: Arc::new(
                rim_services::server_member::service::MockServerMemberService::new(),
            ),
            user_service: Arc::new(rim_services::user::service::MockUserService::new()),
            workspace_state_service: Arc::new(
                rim_services::workspace_state::service::MockWorkspaceStateService::new(),
            ),
            audit_log_service: None,
            notification_preference_service: Arc::new(preferences),
            moderation_service: Arc::new(
                rim_services::moderation::service::MockModerationService::no_timeouts(),
            ),
            notification_service: Arc::new(notification),
        }
    }

    fn context(workspace: ServerId, conversation: Option<ConversationId>) -> CommandContextDto {
        CommandContextDto {
            workspace_id: Some(workspace),
            conversation_id: conversation,
            thread_id: None,
            selected_text: None,
            selected_message_id: None,
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        }
    }

    fn args(value: Value) -> serde_json::Map<String, Value> {
        match value {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        }
    }

    fn settings(server_id: ServerId, pause: DndPause) -> NotificationSettingsDto {
        NotificationSettingsDto {
            server_id,
            default_level: NotificationLevel::All,
            keywords: vec![],
            dnd: None,
            dnd_pause: pause,
            digest: rim_core::notification::model::DigestFrequency::Off,
            reactions: true,
        }
    }

    #[tokio::test]
    async fn set_level_uses_the_focused_conversation() {
        let workspace = ServerId::new();
        let conversation = ConversationId::new();
        let mut notification = MockNotificationService::new();
        notification
            .expect_set_conversation_level()
            .withf(move |cmd| {
                cmd.server_id == workspace
                    && cmd.conversation_id == conversation
                    && cmd.level == Some(NotificationLevel::Mentions)
            })
            .times(1)
            .returning(move |cmd| {
                Box::pin(async move {
                    Ok(ConversationNotificationLevelDto {
                        conversation_id: cmd.conversation_id,
                        level: cmd.level,
                        effective_level: NotificationLevel::Mentions,
                        muted_until: None,
                    })
                })
            });
        let result = set_level(
            &services(notification),
            Actor::new(UserId::new()),
            args(serde_json::json!({ "level": "Mentions" })),
            Some(context(workspace, Some(conversation))),
        )
        .await
        .expect("set level");
        let payload = result.payload.expect("payload");
        assert_eq!(payload["level"], "mentions");
        assert_eq!(payload["effective_level"], "mentions");
    }

    #[tokio::test]
    async fn set_level_default_clears_the_override() {
        let workspace = ServerId::new();
        let conversation = ConversationId::new();
        let mut notification = MockNotificationService::new();
        notification
            .expect_set_conversation_level()
            .withf(|cmd| cmd.level.is_none())
            .times(1)
            .returning(move |cmd| {
                Box::pin(async move {
                    Ok(ConversationNotificationLevelDto {
                        conversation_id: cmd.conversation_id,
                        level: None,
                        effective_level: NotificationLevel::All,
                        muted_until: None,
                    })
                })
            });
        let result = set_level(
            &services(notification),
            Actor::new(UserId::new()),
            args(serde_json::json!({ "level": "default" })),
            Some(context(workspace, Some(conversation))),
        )
        .await
        .expect("set level");
        assert_eq!(result.payload.expect("payload")["level"], Value::Null);
    }

    #[tokio::test]
    async fn set_level_rejects_unknown_levels_without_calling_the_service() {
        let mut notification = MockNotificationService::new();
        notification.expect_set_conversation_level().never();
        let err = set_level(
            &services(notification),
            Actor::new(UserId::new()),
            args(serde_json::json!({ "level": "loud" })),
            Some(context(ServerId::new(), Some(ConversationId::new()))),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn dnd_modes_parse() {
        let now = chrono::Utc::now();
        assert_eq!(
            parse_pause(&args(serde_json::json!({ "mode": "on" })), now).ok(),
            Some(DndPause::Indefinite)
        );
        assert_eq!(
            parse_pause(&args(serde_json::json!({ "mode": "OFF" })), now).ok(),
            Some(DndPause::Off)
        );
        assert_eq!(
            parse_pause(
                &args(serde_json::json!({ "mode": "until", "minutes": "90" })),
                now
            )
            .ok(),
            Some(DndPause::Until(now + chrono::Duration::minutes(90)))
        );
        let until = parse_pause(
            &args(serde_json::json!({ "mode": "until", "until": "2030-01-01T17:00:00Z" })),
            now,
        )
        .expect("until");
        assert_eq!(
            until,
            DndPause::Until(
                chrono::DateTime::parse_from_rfc3339("2030-01-01T17:00:00Z")
                    .expect("ts")
                    .with_timezone(&chrono::Utc)
            )
        );
        for bad in [
            serde_json::json!({ "mode": "until" }),
            serde_json::json!({ "mode": "until", "minutes": 0 }),
            serde_json::json!({ "mode": "until", "minutes": 20000 }),
            serde_json::json!({ "mode": "until", "until": "tomorrow" }),
            serde_json::json!({ "mode": "sometimes" }),
            serde_json::json!({}),
        ] {
            assert!(parse_pause(&args(bad.clone()), now).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn dnd_on_sets_an_indefinite_pause() {
        let workspace = ServerId::new();
        let mut notification = MockNotificationService::new();
        notification
            .expect_set_dnd_pause()
            .withf(move |cmd| cmd.server_id == workspace && cmd.pause == DndPause::Indefinite)
            .times(1)
            .returning(move |cmd| {
                Box::pin(async move { Ok(settings(cmd.server_id, DndPause::Indefinite)) })
            });
        let result = dnd(
            &services(notification),
            Actor::new(UserId::new()),
            args(serde_json::json!({ "mode": "on" })),
            Some(context(workspace, None)),
        )
        .await
        .expect("dnd");
        assert_eq!(result.message, "do not disturb on");
        assert_eq!(result.payload.expect("payload")["paused"], true);
    }

    #[tokio::test]
    async fn mark_all_read_marks_everything_up_to_now() {
        let workspace = ServerId::new();
        let mut notification = MockNotificationService::new();
        notification
            .expect_mark_read()
            .withf(move |cmd| {
                cmd.server_id == workspace
                    && matches!(cmd.target, MarkNotificationsReadTarget::AllBefore(_))
            })
            .times(1)
            .returning(|_| Box::pin(async { Ok(4) }));
        let result = mark_all_read(
            &services(notification),
            Actor::new(UserId::new()),
            serde_json::Map::new(),
            Some(context(workspace, None)),
        )
        .await
        .expect("mark all read");
        assert_eq!(result.payload.expect("payload")["marked"], 4);
    }
    fn at(rfc3339: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(rfc3339)
            .expect("ts")
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn mute_durations_parse() {
        let now = at("2030-01-01T12:00:00Z");
        let end = |value: Value| parse_mute_end(&args(value), now);
        assert_eq!(
            end(serde_json::json!({ "duration": "1h" })).ok(),
            Some(MuteEnd::At(at("2030-01-01T13:00:00Z")))
        );
        assert_eq!(
            end(serde_json::json!({ "duration": "8H" })).ok(),
            Some(MuteEnd::At(at("2030-01-01T20:00:00Z")))
        );
        assert_eq!(
            end(serde_json::json!({ "duration": "until_tomorrow" })).ok(),
            Some(MuteEnd::TomorrowMorning)
        );
        assert_eq!(
            end(serde_json::json!({ "duration": "forever" })).ok(),
            Some(MuteEnd::Never)
        );
        assert_eq!(
            end(serde_json::json!({ "duration": "until", "until": "2030-01-02T08:00:00+01:00" }))
                .ok(),
            Some(MuteEnd::At(at("2030-01-02T07:00:00Z")))
        );
        for bad in [
            serde_json::json!({}),
            serde_json::json!({ "duration": "2h" }),
            serde_json::json!({ "duration": "until" }),
            serde_json::json!({ "duration": "until", "until": "tomorrow" }),
            serde_json::json!({ "duration": "until", "until": "2030-01-01T12:00:00Z" }),
            serde_json::json!({ "duration": "until", "until": "2029-12-31T12:00:00Z" }),
            serde_json::json!({ "duration": "1h", "until": "2030-01-02T08:00:00Z" }),
        ] {
            assert!(
                matches!(end(bad.clone()), Err(ServiceError::Validation(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn tomorrow_morning_is_nine_on_the_next_local_day() {
        // 23:30 UTC on 1 January is already 2 January in Prague (UTC+1), so tomorrow is the 3rd.
        assert_eq!(
            tomorrow_morning(at("2030-01-01T23:30:00Z"), chrono_tz::Europe::Prague),
            at("2030-01-03T08:00:00Z")
        );
        // 03:00 UTC on 2 January is still 1 January in Chicago (UTC-6).
        assert_eq!(
            tomorrow_morning(at("2030-01-02T03:00:00Z"), chrono_tz::America::Chicago),
            at("2030-01-02T15:00:00Z")
        );
        assert_eq!(
            tomorrow_morning(at("2030-01-01T23:30:00Z"), chrono_tz::UTC),
            at("2030-01-02T09:00:00Z")
        );
        // Summer time starts overnight: 09:00 the next day is in CEST (UTC+2).
        assert_eq!(
            tomorrow_morning(at("2030-03-30T12:00:00Z"), chrono_tz::Europe::Prague),
            at("2030-03-31T07:00:00Z")
        );
    }

    fn mutes(
        workspace: ServerId,
        conversation: ConversationId,
        check: impl Fn(Option<chrono::DateTime<chrono::Utc>>) -> bool + Send + Sync + 'static,
    ) -> MockNotificationPreferenceService {
        let mut preferences = MockNotificationPreferenceService::new();
        preferences
            .expect_mute_conversation()
            .withf(move |cmd| {
                cmd.server_id == workspace
                    && cmd.conversation_id == conversation
                    && check(cmd.muted_until)
            })
            .times(1)
            .returning(|_| Box::pin(async { Ok(true) }));
        preferences
    }

    #[tokio::test]
    async fn mute_for_an_hour_mutes_the_focused_conversation() {
        let workspace = ServerId::new();
        let conversation = ConversationId::new();
        let before = chrono::Utc::now();
        let preferences = mutes(workspace, conversation, move |until| {
            until.is_some_and(|until| {
                let minutes = (until - before).num_minutes();
                (59..=61).contains(&minutes)
            })
        });
        let mut notification = MockNotificationService::new();
        notification.expect_get_settings().never();
        let result = mute_conversation(
            &services_with(notification, preferences),
            Actor::new(UserId::new()),
            args(serde_json::json!({ "duration": "1h" })),
            Some(context(workspace, Some(conversation))),
        )
        .await
        .expect("mute");
        let payload = result.payload.expect("payload");
        assert_eq!(payload["conversation_id"], conversation.to_string());
        assert!(payload["muted_until"].is_string());
    }

    #[tokio::test]
    async fn mute_forever_sets_no_end() {
        let workspace = ServerId::new();
        let conversation = ConversationId::new();
        let preferences = mutes(workspace, conversation, |until| until.is_none());
        let result = mute_conversation(
            &services_with(MockNotificationService::new(), preferences),
            Actor::new(UserId::new()),
            args(serde_json::json!({
                "duration": "forever",
                "conversation_id": conversation.to_string(),
            })),
            Some(context(workspace, Some(ConversationId::new()))),
        )
        .await
        .expect("mute forever");
        assert_eq!(result.payload.expect("payload")["muted_until"], Value::Null);
    }

    #[tokio::test]
    async fn mute_until_a_time_ends_then() {
        let workspace = ServerId::new();
        let conversation = ConversationId::new();
        let preferences = mutes(workspace, conversation, |until| {
            until == Some(at("2099-06-01T07:00:00Z"))
        });
        mute_conversation(
            &services_with(MockNotificationService::new(), preferences),
            Actor::new(UserId::new()),
            args(serde_json::json!({ "duration": "until", "until": "2099-06-01T09:00:00+02:00" })),
            Some(context(workspace, Some(conversation))),
        )
        .await
        .expect("mute until");
    }

    fn settings_with_window(
        server_id: ServerId,
        timezone: Option<&str>,
    ) -> NotificationSettingsDto {
        NotificationSettingsDto {
            dnd: timezone.map(|tz| rim_core::notification::model::DndWindow {
                start: chrono::NaiveTime::from_hms_opt(22, 0, 0).expect("time"),
                end: chrono::NaiveTime::from_hms_opt(7, 0, 0).expect("time"),
                timezone: tz.to_string(),
            }),
            ..settings(server_id, DndPause::Off)
        }
    }

    #[tokio::test]
    async fn mute_until_tomorrow_uses_the_do_not_disturb_window_timezone() {
        for (timezone, zone) in [
            (Some("Asia/Tokyo"), chrono_tz::Asia::Tokyo),
            (None, chrono_tz::UTC),
        ] {
            let workspace = ServerId::new();
            let conversation = ConversationId::new();
            let before = chrono::Utc::now();
            // 09:00 local, between 9 and 33 hours ahead (an hour either way across a clock
            // change).
            let preferences = mutes(workspace, conversation, move |until| {
                until.is_some_and(|until| {
                    until.with_timezone(&zone).time()
                        == chrono::NaiveTime::from_hms_opt(9, 0, 0).expect("time")
                        && (8..=34).contains(&(until - before).num_hours())
                })
            });
            let mut notification = MockNotificationService::new();
            let tz = timezone.map(str::to_string);
            notification
                .expect_get_settings()
                .withf(move |query| query.server_id == workspace)
                .times(1)
                .returning(move |query| {
                    let tz = tz.clone();
                    Box::pin(
                        async move { Ok(settings_with_window(query.server_id, tz.as_deref())) },
                    )
                });
            mute_conversation(
                &services_with(notification, preferences),
                Actor::new(UserId::new()),
                args(serde_json::json!({ "duration": "until_tomorrow" })),
                Some(context(workspace, Some(conversation))),
            )
            .await
            .expect("mute until tomorrow");
        }
    }

    #[tokio::test]
    async fn mute_passes_the_services_refusal_through() {
        let mut preferences = MockNotificationPreferenceService::new();
        preferences
            .expect_mute_conversation()
            .times(1)
            .returning(|_| Box::pin(async { Err(ServiceError::NotFound) }));
        let err = mute_conversation(
            &services_with(MockNotificationService::new(), preferences),
            Actor::new(UserId::new()),
            args(serde_json::json!({ "duration": "8h" })),
            Some(context(ServerId::new(), Some(ConversationId::new()))),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound), "{err:?}");
    }

    #[tokio::test]
    async fn mute_without_a_conversation_or_with_a_bad_duration_calls_nothing() {
        let mut preferences = MockNotificationPreferenceService::new();
        preferences.expect_mute_conversation().never();
        let services = services_with(MockNotificationService::new(), preferences);
        for (value, conversation) in [
            (serde_json::json!({ "duration": "1h" }), None),
            (
                serde_json::json!({ "duration": "soon" }),
                Some(ConversationId::new()),
            ),
        ] {
            let err = mute_conversation(
                &services,
                Actor::new(UserId::new()),
                args(value),
                Some(context(ServerId::new(), conversation)),
            )
            .await
            .unwrap_err();
            assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
        }
    }

    #[tokio::test]
    async fn unmute_unmutes_the_focused_conversation() {
        let workspace = ServerId::new();
        let conversation = ConversationId::new();
        let mut preferences = MockNotificationPreferenceService::new();
        preferences
            .expect_unmute_conversation()
            .withf(move |cmd| cmd.server_id == workspace && cmd.conversation_id == conversation)
            .times(1)
            .returning(|_| Box::pin(async { Ok(true) }));
        let result = unmute_conversation(
            &services_with(MockNotificationService::new(), preferences),
            Actor::new(UserId::new()),
            serde_json::Map::new(),
            Some(context(workspace, Some(conversation))),
        )
        .await
        .expect("unmute");
        assert_eq!(
            result.payload.expect("payload")["conversation_id"],
            conversation.to_string()
        );
    }

    #[tokio::test]
    async fn unmute_passes_the_services_refusal_through() {
        let mut preferences = MockNotificationPreferenceService::new();
        preferences
            .expect_unmute_conversation()
            .times(1)
            .returning(|_| Box::pin(async { Err(ServiceError::NotFound) }));
        let err = unmute_conversation(
            &services_with(MockNotificationService::new(), preferences),
            Actor::new(UserId::new()),
            serde_json::Map::new(),
            Some(context(ServerId::new(), Some(ConversationId::new()))),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound), "{err:?}");
    }
}
