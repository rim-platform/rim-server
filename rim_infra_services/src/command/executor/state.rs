use serde_json::Value;

use rim_services::{Actor, ServiceResult};

use crate::command::CommandServices;
use rim_services::command::dto::{CommandContextDto, CommandResultDto, CommandResultStatusDto};

use super::{optional_string_arg, parse_scope_arg, require_string_arg, resolve_workspace_id};

pub(super) async fn get(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id::<rim_core::server::model::ServerId>(
        &args,
        context.as_ref(),
        "server_id",
    )?;
    let key = require_string_arg(&args, "key")?;
    let scope = parse_scope_arg(&args)?;

    let entry = services
        .workspace_state_service
        .get(
            rim_services::workspace_state::request::GetWorkspaceStateQuery {
                actor,
                server_id,
                key,
                scope,
            },
        )
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "state retrieved".to_string(),
        payload: entry.map(|e| {
            let scope_str = match e.scope {
                rim_services::workspace_state::dto::WorkspaceStateScopeDto::Server => "server",
                rim_services::workspace_state::dto::WorkspaceStateScopeDto::User => "user",
            };
            serde_json::json!({
                "key": e.key,
                "value": e.value,
                "version": e.version,
                "scope": scope_str,
            })
        }),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

pub(super) async fn set(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id::<rim_core::server::model::ServerId>(
        &args,
        context.as_ref(),
        "server_id",
    )?;
    let key = require_string_arg(&args, "key")?;
    let value_str = require_string_arg(&args, "value")?;
    let value: serde_json::Value = match serde_json::from_str(&value_str) {
        Ok(v) => v,
        Err(_) => serde_json::Value::String(value_str),
    };
    let scope = parse_scope_arg(&args)?;
    let ttl_seconds = optional_string_arg(&args, "ttl").and_then(|s| s.parse::<i64>().ok());

    let entry = services
        .workspace_state_service
        .set(
            rim_services::workspace_state::request::SetWorkspaceStateCmd {
                actor,
                server_id,
                key,
                value,
                scope,
                ttl_seconds,
            },
        )
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "state set".to_string(),
        payload: Some(serde_json::json!({
            "key": entry.key,
            "value": entry.value,
            "version": entry.version,
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

pub(super) async fn delete(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id::<rim_core::server::model::ServerId>(
        &args,
        context.as_ref(),
        "server_id",
    )?;
    let key = require_string_arg(&args, "key")?;
    let scope = parse_scope_arg(&args)?;

    let deleted = services
        .workspace_state_service
        .delete(
            rim_services::workspace_state::request::DeleteWorkspaceStateCmd {
                actor,
                server_id,
                key: key.clone(),
                scope,
            },
        )
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: if deleted {
            "state deleted".to_string()
        } else {
            "state key not found".to_string()
        },
        payload: Some(serde_json::json!({
            "key": key,
            "deleted": deleted,
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

pub(super) async fn list(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id::<rim_core::server::model::ServerId>(
        &args,
        context.as_ref(),
        "server_id",
    )?;
    let prefix = optional_string_arg(&args, "prefix").unwrap_or_default();
    let scope = parse_scope_arg(&args)?;
    let limit = optional_string_arg(&args, "limit").and_then(|s| s.parse::<u64>().ok());

    let entries = services
        .workspace_state_service
        .list(
            rim_services::workspace_state::request::ListWorkspaceStateQuery {
                actor,
                server_id,
                prefix,
                scope,
                limit,
            },
        )
        .await?;

    let items: Vec<serde_json::Value> = entries
        .into_iter()
        .map(|e| {
            serde_json::json!({
                "key": e.key,
                "value": e.value,
                "version": e.version,
            })
        })
        .collect();

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: format!("{} entries found", items.len()),
        payload: Some(serde_json::json!({ "entries": items })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

pub(super) async fn incr(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id::<rim_core::server::model::ServerId>(
        &args,
        context.as_ref(),
        "server_id",
    )?;
    let key = require_string_arg(&args, "key")?;
    let delta = optional_string_arg(&args, "delta")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(1);
    let scope = parse_scope_arg(&args)?;

    let entry = services
        .workspace_state_service
        .increment(
            rim_services::workspace_state::request::IncrementWorkspaceStateCmd {
                actor,
                server_id,
                key,
                scope,
                delta,
            },
        )
        .await?;

    Ok(CommandResultDto {
        status: CommandResultStatusDto::Success,
        message: "state incremented".to_string(),
        payload: Some(serde_json::json!({
            "key": entry.key,
            "value": entry.value,
            "version": entry.version,
        })),
        follow_up_commands: vec![],
        pipeline_stages: None,
        confirmation_required: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::Actor;
    use rim_services::auth::service::MockAuthService;
    use rim_services::capability::service::MockCapabilityService;
    use rim_services::command::dto::{CommandContextDto, CommandResultStatusDto};
    use rim_services::conversation::service::MockConversationService;
    use rim_services::message::service::MockMessageService;
    use rim_services::pin::service::MockPinService;
    use rim_services::read_state::service::MockReadStateService;
    use rim_services::server_member::service::MockServerMemberService;
    use rim_services::user::service::MockUserService;
    use rim_services::workspace_state::dto::{WorkspaceStateDto, WorkspaceStateScopeDto};
    use rim_services::workspace_state::service::MockWorkspaceStateService;

    use crate::command::CommandServices;

    fn actor() -> Actor {
        Actor::new(UserId::new())
    }

    fn ctx_with(ws: ServerId) -> CommandContextDto {
        CommandContextDto {
            workspace_id: Some(ws),
            conversation_id: None,
            thread_id: None,
            selected_text: None,
            selected_message_id: None,
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        }
    }

    fn make_services(ws_state: MockWorkspaceStateService) -> CommandServices {
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
            workspace_state_service: Arc::new(ws_state),
            audit_log_service: None,
            incident_service: std::sync::Arc::new(
                rim_services::incident::service::MockIncidentService::new(),
            ),
            oncall_service: std::sync::Arc::new(
                rim_services::oncall::service::MockOnCallService::new(),
            ),
        }
    }

    fn fake_entry(ws: ServerId, key: &str) -> WorkspaceStateDto {
        WorkspaceStateDto {
            id: rim_core::workspace_state::model::WorkspaceStateId::new(),
            server_id: ws,
            key: key.to_string(),
            value: serde_json::json!("val"),
            scope: WorkspaceStateScopeDto::Server,
            user_id: None,
            version: 1,
            expires_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn args_with_ws_and_key(ws: ServerId, key: &str) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        m.insert(
            "server_id".to_string(),
            serde_json::Value::String(ws.to_string()),
        );
        m.insert(
            "key".to_string(),
            serde_json::Value::String(key.to_string()),
        );
        m
    }

    fn args_with_key(key: &str) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        m.insert(
            "key".to_string(),
            serde_json::Value::String(key.to_string()),
        );
        m
    }

    // ── state.get ────────────────────────────────────────────────────────────

    /// 1. AI path: args carry server_id.
    #[tokio::test]
    async fn get_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_get()
            .withf(move |q| q.server_id == ws_check)
            .return_once(|_| Box::pin(async { Ok(None) }));

        let svc = make_services(wss);
        let result = super::get(&svc, actor(), args_with_ws_and_key(ws, "k"), None)
            .await
            .expect("state.get with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace.
    #[tokio::test]
    async fn get_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_get()
            .withf(move |q| q.server_id == ws_check)
            .return_once(|_| Box::pin(async { Ok(None) }));

        let svc = make_services(wss);
        let result = super::get(&svc, actor(), args_with_key("k"), Some(ctx_with(ws)))
            .await
            .expect("state.get with context workspace must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn get_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_get()
            .withf(move |q| q.server_id == arg_ws_check)
            .return_once(|_| Box::pin(async { Ok(None) }));

        let svc = make_services(wss);
        let result = super::get(
            &svc,
            actor(),
            args_with_ws_and_key(arg_ws, "k"),
            Some(ctx_with(ctx_ws)),
        )
        .await
        .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation error mentions both paths.
    #[tokio::test]
    async fn get_both_absent_returns_validation() {
        let svc = make_services(MockWorkspaceStateService::new());
        let err = super::get(&svc, actor(), args_with_key("k"), None)
            .await
            .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
        if let rim_services::ServiceError::Validation(msg) = err {
            assert!(msg.contains("server_id"));
            assert!(msg.contains("context.workspace_id"));
        }
    }

    // ── state.set ────────────────────────────────────────────────────────────

    fn args_with_ws_key_value(
        ws: ServerId,
        key: &str,
        value: &str,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut m = args_with_ws_and_key(ws, key);
        m.insert(
            "value".to_string(),
            serde_json::Value::String(value.to_string()),
        );
        m
    }

    fn args_with_key_value(key: &str, value: &str) -> serde_json::Map<String, serde_json::Value> {
        let mut m = args_with_key(key);
        m.insert(
            "value".to_string(),
            serde_json::Value::String(value.to_string()),
        );
        m
    }

    /// 1. AI path: args carry server_id.
    #[tokio::test]
    async fn set_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_set()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_entry(ws, "k")) }));

        let svc = make_services(wss);
        let result = super::set(&svc, actor(), args_with_ws_key_value(ws, "k", "v"), None)
            .await
            .expect("state.set with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace.
    #[tokio::test]
    async fn set_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_set()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_entry(ws, "k")) }));

        let svc = make_services(wss);
        let result = super::set(
            &svc,
            actor(),
            args_with_key_value("k", "v"),
            Some(ctx_with(ws)),
        )
        .await
        .expect("state.set with context workspace must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn set_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_set()
            .withf(move |cmd| cmd.server_id == arg_ws_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_entry(arg_ws, "k")) }));

        let svc = make_services(wss);
        let result = super::set(
            &svc,
            actor(),
            args_with_ws_key_value(arg_ws, "k", "v"),
            Some(ctx_with(ctx_ws)),
        )
        .await
        .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation error mentions both paths.
    #[tokio::test]
    async fn set_both_absent_returns_validation() {
        let svc = make_services(MockWorkspaceStateService::new());
        let err = super::set(&svc, actor(), args_with_key_value("k", "v"), None)
            .await
            .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
        if let rim_services::ServiceError::Validation(msg) = err {
            assert!(msg.contains("server_id"));
            assert!(msg.contains("context.workspace_id"));
        }
    }

    // ── state.delete ─────────────────────────────────────────────────────────

    /// 1. AI path: args carry server_id.
    #[tokio::test]
    async fn delete_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_delete()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(|_| Box::pin(async { Ok(true) }));

        let svc = make_services(wss);
        let result = super::delete(&svc, actor(), args_with_ws_and_key(ws, "k"), None)
            .await
            .expect("state.delete with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace.
    #[tokio::test]
    async fn delete_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_delete()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(|_| Box::pin(async { Ok(true) }));

        let svc = make_services(wss);
        let result = super::delete(&svc, actor(), args_with_key("k"), Some(ctx_with(ws)))
            .await
            .expect("state.delete with context workspace must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn delete_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_delete()
            .withf(move |cmd| cmd.server_id == arg_ws_check)
            .return_once(|_| Box::pin(async { Ok(true) }));

        let svc = make_services(wss);
        let result = super::delete(
            &svc,
            actor(),
            args_with_ws_and_key(arg_ws, "k"),
            Some(ctx_with(ctx_ws)),
        )
        .await
        .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation error mentions both paths.
    #[tokio::test]
    async fn delete_both_absent_returns_validation() {
        let svc = make_services(MockWorkspaceStateService::new());
        let err = super::delete(&svc, actor(), args_with_key("k"), None)
            .await
            .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
        if let rim_services::ServiceError::Validation(msg) = err {
            assert!(msg.contains("server_id"));
            assert!(msg.contains("context.workspace_id"));
        }
    }

    // ── state.list ───────────────────────────────────────────────────────────

    /// 1. AI path: args carry server_id.
    #[tokio::test]
    async fn list_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_list()
            .withf(move |q| q.server_id == ws_check)
            .return_once(|_| Box::pin(async { Ok(vec![]) }));

        let svc = make_services(wss);
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(ws.to_string()),
        );
        let result = super::list(&svc, actor(), args, None)
            .await
            .expect("state.list with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace.
    #[tokio::test]
    async fn list_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_list()
            .withf(move |q| q.server_id == ws_check)
            .return_once(|_| Box::pin(async { Ok(vec![]) }));

        let svc = make_services(wss);
        let result = super::list(&svc, actor(), serde_json::Map::new(), Some(ctx_with(ws)))
            .await
            .expect("state.list with context workspace must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn list_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_list()
            .withf(move |q| q.server_id == arg_ws_check)
            .return_once(|_| Box::pin(async { Ok(vec![]) }));

        let svc = make_services(wss);
        let mut args = serde_json::Map::new();
        args.insert(
            "server_id".to_string(),
            serde_json::Value::String(arg_ws.to_string()),
        );
        let result = super::list(&svc, actor(), args, Some(ctx_with(ctx_ws)))
            .await
            .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation error mentions both paths.
    #[tokio::test]
    async fn list_both_absent_returns_validation() {
        let svc = make_services(MockWorkspaceStateService::new());
        let err = super::list(&svc, actor(), serde_json::Map::new(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
        if let rim_services::ServiceError::Validation(msg) = err {
            assert!(msg.contains("server_id"));
            assert!(msg.contains("context.workspace_id"));
        }
    }

    // ── state.incr ───────────────────────────────────────────────────────────

    /// 1. AI path: args carry server_id.
    #[tokio::test]
    async fn incr_args_present_uses_arg_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_increment()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_entry(ws, "counter")) }));

        let svc = make_services(wss);
        let result = super::incr(&svc, actor(), args_with_ws_and_key(ws, "counter"), None)
            .await
            .expect("state.incr with arg must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 2. Palette path: no server_id in args, context carries workspace.
    #[tokio::test]
    async fn incr_args_absent_uses_context_value() {
        let ws = ServerId::new();
        let ws_check = ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_increment()
            .withf(move |cmd| cmd.server_id == ws_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_entry(ws, "counter")) }));

        let svc = make_services(wss);
        let result = super::incr(&svc, actor(), args_with_key("counter"), Some(ctx_with(ws)))
            .await
            .expect("state.incr with context workspace must succeed");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 3. Both supplied — arg wins.
    #[tokio::test]
    async fn incr_args_present_and_context_present_args_wins() {
        let arg_ws = ServerId::new();
        let ctx_ws = ServerId::new();
        assert_ne!(arg_ws, ctx_ws);

        let arg_ws_check = arg_ws;
        let mut wss = MockWorkspaceStateService::new();
        wss.expect_increment()
            .withf(move |cmd| cmd.server_id == arg_ws_check)
            .return_once(move |_| Box::pin(async move { Ok(fake_entry(arg_ws, "counter")) }));

        let svc = make_services(wss);
        let result = super::incr(
            &svc,
            actor(),
            args_with_ws_and_key(arg_ws, "counter"),
            Some(ctx_with(ctx_ws)),
        )
        .await
        .expect("arg wins when both present");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// 4. Neither — Validation error mentions both paths.
    #[tokio::test]
    async fn incr_both_absent_returns_validation() {
        let svc = make_services(MockWorkspaceStateService::new());
        let err = super::incr(&svc, actor(), args_with_key("counter"), None)
            .await
            .unwrap_err();
        assert!(matches!(err, rim_services::ServiceError::Validation(_)));
        if let rim_services::ServiceError::Validation(msg) = err {
            assert!(msg.contains("server_id"));
            assert!(msg.contains("context.workspace_id"));
        }
    }
}
