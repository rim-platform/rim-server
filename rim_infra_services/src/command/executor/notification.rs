use serde_json::Value;

use rim_core::notification::model::{DndPause, NotificationLevel};
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};
use rim_services::notification::command::{
    MarkNotificationsReadCmd, MarkNotificationsReadTarget, SetConversationNotificationLevelCmd,
    SetDndPauseCmd,
};
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

    use super::*;

    fn services(notification: MockNotificationService) -> CommandServices {
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
}
