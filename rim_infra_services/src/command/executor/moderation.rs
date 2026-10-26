use serde_json::Value;

use rim_core::conversation::model::PostingPolicy;
use rim_core::server_member::model::ServerMemberRole;
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};
use rim_services::moderation::command::{
    BanMemberCmd, KickMemberCmd, ReportMessageCmd, SetChannelPostingCmd, SetMemberRoleCmd,
    SetSlowModeCmd, TimeoutMemberCmd, UnbanMemberCmd,
};
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::command::CommandServices;

use super::{
    optional_bool_arg, optional_integer_arg, optional_string_arg, parse_uuid_arg,
    require_string_arg, resolve_conversation_id, resolve_workspace_id,
};

/// Longest timeout the `admin.timeout` command accepts, in minutes (30 days); the service
/// enforces the same bound.
const MAX_TIMEOUT_MINUTES: i64 = 30 * 24 * 60;

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

fn target(
    args: &serde_json::Map<String, Value>,
    context: Option<&CommandContextDto>,
) -> ServiceResult<(
    rim_core::server::model::ServerId,
    rim_core::user::model::UserId,
)> {
    Ok((
        resolve_workspace_id(args, context, "server_id")?,
        parse_uuid_arg(args, "user_id")?,
    ))
}

pub(super) async fn kick(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let (server_id, user_id) = target(&args, context.as_ref())?;
    services
        .moderation_service
        .kick(KickMemberCmd {
            actor,
            server_id,
            user_id,
            reason: optional_string_arg(&args, "reason"),
        })
        .await?;
    Ok(success(
        "member kicked",
        serde_json::json!({ "user_id": user_id.to_string() }),
    ))
}

pub(super) async fn ban(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let (server_id, user_id) = target(&args, context.as_ref())?;
    let expires_at = match optional_string_arg(&args, "expires_at").filter(|s| !s.trim().is_empty())
    {
        Some(raw) => Some(
            chrono::DateTime::parse_from_rfc3339(raw.trim())
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .map_err(|_| {
                    ServiceError::Validation(
                        "invalid RFC 3339 timestamp arg `expires_at`".to_string(),
                    )
                })?,
        ),
        None => None,
    };
    let ban = services
        .moderation_service
        .ban(BanMemberCmd {
            actor,
            server_id,
            user_id,
            reason: optional_string_arg(&args, "reason"),
            expires_at,
        })
        .await?;
    Ok(success(
        "member banned",
        serde_json::json!({
            "user_id": user_id.to_string(),
            "expires_at": ban.expires_at.map(|t| t.to_rfc3339()),
        }),
    ))
}

pub(super) async fn unban(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let (server_id, user_id) = target(&args, context.as_ref())?;
    let existed = services
        .moderation_service
        .unban(UnbanMemberCmd {
            actor,
            server_id,
            user_id,
        })
        .await?;
    Ok(success(
        if existed {
            "ban lifted"
        } else {
            "user was not banned"
        },
        serde_json::json!({ "user_id": user_id.to_string(), "was_banned": existed }),
    ))
}

pub(super) async fn timeout(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let (server_id, user_id) = target(&args, context.as_ref())?;
    let minutes = optional_integer_arg(&args, "minutes")
        .filter(|m| (1..=MAX_TIMEOUT_MINUTES).contains(m))
        .ok_or_else(|| {
            ServiceError::Validation(format!(
                "arg `minutes` must be an integer from 1 to {MAX_TIMEOUT_MINUTES}"
            ))
        })?;
    let timeout = services
        .moderation_service
        .timeout(TimeoutMemberCmd {
            actor,
            server_id,
            user_id,
            until: chrono::Utc::now() + chrono::Duration::minutes(minutes),
            reason: optional_string_arg(&args, "reason"),
        })
        .await?;
    Ok(success(
        "member timed out",
        serde_json::json!({
            "user_id": user_id.to_string(),
            "until": timeout.until.to_rfc3339(),
        }),
    ))
}

pub(super) async fn set_role(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let (server_id, user_id) = target(&args, context.as_ref())?;
    let role = require_string_arg(&args, "role")?
        .trim()
        .to_ascii_lowercase()
        .parse::<ServerMemberRole>()
        .map_err(|e| ServiceError::Validation(e.to_string()))?;
    let member = services
        .moderation_service
        .set_member_role(SetMemberRoleCmd {
            actor,
            server_id,
            user_id,
            role,
        })
        .await?;
    Ok(success(
        "role changed",
        serde_json::json!({
            "user_id": user_id.to_string(),
            "role": member.member_role.as_str(),
        }),
    ))
}

pub(super) async fn set_slow_mode(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id(&args, context.as_ref(), "server_id")?;
    let conversation_id = resolve_conversation_id(&args, context.as_ref(), "conversation_id")?;
    let seconds = optional_integer_arg(&args, "seconds")
        .and_then(|s| i32::try_from(s).ok())
        .ok_or_else(|| ServiceError::Validation("missing integer arg `seconds`".to_string()))?;
    let conversation = services
        .moderation_service
        .set_slow_mode(SetSlowModeCmd {
            actor,
            server_id,
            conversation_id,
            seconds,
        })
        .await?;
    Ok(success(
        if conversation.slow_mode_seconds == 0 {
            "slow mode off"
        } else {
            "slow mode on"
        },
        serde_json::json!({
            "conversation_id": conversation.id.to_string(),
            "slow_mode_seconds": conversation.slow_mode_seconds,
        }),
    ))
}

pub(super) async fn set_channel_posting(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id(&args, context.as_ref(), "server_id")?;
    let conversation_id = resolve_conversation_id(&args, context.as_ref(), "conversation_id")?;
    let policy = require_string_arg(&args, "policy")?
        .trim()
        .to_ascii_lowercase()
        .parse::<PostingPolicy>()
        .map_err(|_| {
            ServiceError::Validation("policy must be everyone or moderators".to_string())
        })?;
    let allows_threads = optional_bool_arg(&args, "allows_threads").unwrap_or(true);
    let conversation = services
        .moderation_service
        .set_channel_posting(SetChannelPostingCmd {
            actor,
            server_id,
            conversation_id,
            policy,
            allows_threads,
        })
        .await?;
    Ok(success(
        match conversation.posting_policy {
            PostingPolicy::Everyone => "everyone can post",
            PostingPolicy::Moderators => "only the owner and moderators can post",
        },
        serde_json::json!({
            "conversation_id": conversation.id.to_string(),
            "posting_policy": conversation.posting_policy.as_str(),
            "posting_allows_threads": conversation.posting_allows_threads,
        }),
    ))
}

pub(super) async fn report_message(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let Some(context) = context else {
        return Err(ServiceError::Validation(
            "command requires context".to_string(),
        ));
    };
    let server_id = context.workspace_id.ok_or_else(|| {
        ServiceError::Validation("command requires workspace context".to_string())
    })?;
    let conversation_id = context.conversation_id.ok_or_else(|| {
        ServiceError::Validation("command requires conversation context".to_string())
    })?;
    let message_id = context.selected_message_id.ok_or_else(|| {
        ServiceError::Validation("command requires selected message context".to_string())
    })?;
    let reason = require_string_arg(&args, "reason")?;
    let report = services
        .moderation_service
        .report_message(ReportMessageCmd {
            actor,
            server_id,
            conversation_id,
            message_id,
            reason,
        })
        .await?;
    Ok(success(
        "message reported",
        serde_json::json!({
            "report_id": report.id.to_string(),
            "message_id": message_id.to_string(),
        }),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::model::{ConversationId, ConversationType, PostingPolicy};
    use rim_core::message::model::MessageId;
    use rim_core::moderation::model::{MessageReportId, MessageReportStatus};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::Actor;
    use rim_services::ServiceError;
    use rim_services::auth::service::MockAuthService;
    use rim_services::capability::service::MockCapabilityService;
    use rim_services::command::dto::CommandContextDto;
    use rim_services::conversation::dto::ConversationDto;
    use rim_services::conversation::service::MockConversationService;
    use rim_services::message::service::MockMessageService;
    use rim_services::moderation::dto::{MemberTimeoutDto, MessageReportDto};
    use rim_services::moderation::service::MockModerationService;
    use rim_services::pin::service::MockPinService;
    use rim_services::read_state::service::MockReadStateService;
    use rim_services::server_member::service::MockServerMemberService;
    use rim_services::user::service::MockUserService;
    use rim_services::workspace_state::service::MockWorkspaceStateService;

    use crate::command::CommandServices;

    fn make_services(moderation: MockModerationService) -> CommandServices {
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(MockCapabilityService::new()),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(MockPinService::new()),
            read_state_service: Arc::new(MockReadStateService::new()),
            server_member_service: Arc::new(MockServerMemberService::new()),
            user_service: Arc::new(MockUserService::new()),
            workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
            audit_log_service: None,
            moderation_service: Arc::new(moderation),
            notification_service: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
        }
    }

    fn context(
        workspace: ServerId,
        conversation: Option<ConversationId>,
        message: Option<MessageId>,
    ) -> CommandContextDto {
        CommandContextDto {
            workspace_id: Some(workspace),
            conversation_id: conversation,
            thread_id: None,
            selected_text: None,
            selected_message_id: message,
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        }
    }

    fn object(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[tokio::test]
    async fn timeout_converts_minutes_and_rejects_out_of_range() {
        let workspace = ServerId::new();
        let target = UserId::new();
        let mut moderation = MockModerationService::new();
        moderation
            .expect_timeout()
            .withf(move |cmd| {
                let minutes = (cmd.until - chrono::Utc::now()).num_minutes();
                cmd.user_id == target && (9..=10).contains(&minutes)
            })
            .times(1)
            .returning(|cmd| {
                Box::pin(async move {
                    Ok(MemberTimeoutDto {
                        server_id: cmd.server_id,
                        user_id: cmd.user_id,
                        until: cmd.until,
                        reason: cmd.reason,
                        set_by: Some(cmd.actor.user_id),
                        created_at: chrono::Utc::now(),
                    })
                })
            });
        let services = make_services(moderation);
        let actor = Actor::new(UserId::new());

        let ok = super::timeout(
            &services,
            actor,
            object(serde_json::json!({ "user_id": target.to_string(), "minutes": 10 })),
            Some(context(workspace, None, None)),
        )
        .await
        .expect("timeout");
        assert!(ok.payload.is_some());

        for minutes in [0, 43_201] {
            let err = super::timeout(
                &services,
                actor,
                object(serde_json::json!({ "user_id": target.to_string(), "minutes": minutes })),
                Some(context(workspace, None, None)),
            )
            .await
            .unwrap_err();
            assert!(matches!(err, ServiceError::Validation(_)), "{minutes}");
        }
    }

    #[tokio::test]
    async fn set_role_rejects_unknown_roles() {
        let services = make_services(MockModerationService::new());
        let err = super::set_role(
            &services,
            Actor::new(UserId::new()),
            object(serde_json::json!({ "user_id": UserId::new().to_string(), "role": "admin" })),
            Some(context(ServerId::new(), None, None)),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn set_slow_mode_uses_the_focused_conversation() {
        let workspace = ServerId::new();
        let conversation = ConversationId::new();
        let mut moderation = MockModerationService::new();
        moderation
            .expect_set_slow_mode()
            .withf(move |cmd| cmd.conversation_id == conversation && cmd.seconds == 30)
            .times(1)
            .returning(move |cmd| {
                Box::pin(async move {
                    Ok(ConversationDto {
                        id: cmd.conversation_id,
                        server_id: cmd.server_id,
                        conversation_type: ConversationType::Channel,
                        title: None,
                        topic: None,
                        member_ids: vec![],
                        pending_member_ids: vec![],
                        is_private: false,
                        archived_at: None,
                        created_by: None,
                        slow_mode_seconds: cmd.seconds,
                        posting_policy: PostingPolicy::Everyone,
                        posting_allows_threads: true,
                        encrypted_at: None,
                    })
                })
            });
        let services = make_services(moderation);
        let result = super::set_slow_mode(
            &services,
            Actor::new(UserId::new()),
            object(serde_json::json!({ "seconds": 30 })),
            Some(context(workspace, Some(conversation), None)),
        )
        .await
        .expect("slow mode");
        assert_eq!(result.message, "slow mode on");
    }

    #[tokio::test]
    async fn set_channel_posting_uses_the_focused_conversation() {
        let workspace = ServerId::new();
        let conversation = ConversationId::new();
        let mut moderation = MockModerationService::new();
        moderation
            .expect_set_channel_posting()
            .withf(move |cmd| {
                cmd.conversation_id == conversation
                    && cmd.policy == PostingPolicy::Moderators
                    && cmd.allows_threads
            })
            .times(1)
            .returning(move |cmd| {
                Box::pin(async move {
                    Ok(ConversationDto {
                        id: cmd.conversation_id,
                        server_id: cmd.server_id,
                        conversation_type: ConversationType::Channel,
                        title: None,
                        topic: None,
                        member_ids: vec![],
                        pending_member_ids: vec![],
                        is_private: false,
                        archived_at: None,
                        created_by: None,
                        slow_mode_seconds: 0,
                        encrypted_at: None,
                        posting_policy: cmd.policy,
                        posting_allows_threads: cmd.allows_threads,
                    })
                })
            });
        let services = make_services(moderation);
        let result = super::set_channel_posting(
            &services,
            Actor::new(UserId::new()),
            object(serde_json::json!({ "policy": "Moderators" })),
            Some(context(workspace, Some(conversation), None)),
        )
        .await
        .expect("channel posting");
        assert_eq!(result.message, "only the owner and moderators can post");
        let payload = result.payload.expect("payload");
        assert_eq!(payload["posting_policy"], "moderators");
        assert_eq!(payload["posting_allows_threads"], true);
    }

    #[tokio::test]
    async fn set_channel_posting_rejects_unknown_policies() {
        let services = make_services(MockModerationService::new());
        let err = super::set_channel_posting(
            &services,
            Actor::new(UserId::new()),
            object(serde_json::json!({ "policy": "admins" })),
            Some(context(ServerId::new(), Some(ConversationId::new()), None)),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn report_message_needs_a_selected_message() {
        let workspace = ServerId::new();
        let conversation = ConversationId::new();
        let message = MessageId::new();
        let mut moderation = MockModerationService::new();
        moderation
            .expect_report_message()
            .withf(move |cmd| cmd.message_id == message && cmd.reason == "spam")
            .times(1)
            .returning(|cmd| {
                Box::pin(async move {
                    Ok(MessageReportDto {
                        id: MessageReportId::new(),
                        server_id: cmd.server_id,
                        conversation_id: cmd.conversation_id,
                        message_id: cmd.message_id,
                        reporter_user_id: cmd.actor.user_id,
                        reason: cmd.reason,
                        status: MessageReportStatus::Open,
                        resolved_by: None,
                        resolved_at: None,
                        created_at: chrono::Utc::now(),
                        message_readable: false,
                        message_text: None,
                        message_author_id: None,
                        message_author_principal_id: None,
                        message_sent_at: None,
                        franking_verified_at: None,
                        franked_sender_device_id: None,
                        franked_content: None,
                        franked_content_expires_at: None,
                    })
                })
            });
        let services = make_services(moderation);
        let actor = Actor::new(UserId::new());
        let args = || object(serde_json::json!({ "reason": "spam" }));

        let err = super::report_message(
            &services,
            actor,
            args(),
            Some(context(workspace, Some(conversation), None)),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
        super::report_message(
            &services,
            actor,
            args(),
            Some(context(workspace, Some(conversation), Some(message))),
        )
        .await
        .expect("report");
    }
}
