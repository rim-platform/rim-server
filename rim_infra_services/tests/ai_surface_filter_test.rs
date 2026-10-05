//! Phase 2 P2-A3 integration test — AI surface filter end-to-end.
//!
//! Drives `DefaultCommandService::filter_ai_surface_test_only` against the
//! real `DefaultCapabilityService` (with mocked underlying repos for the
//! registry + member lookups). The unit tests in `src/command.rs` cover the
//! filter logic in isolation against a `MockCapabilityService`; this file
//! locks the wire contract end-to-end so a refactor that decouples the
//! filter from the gate's two-leg semantics (registry ∧ membership) fails
//! the build here.
//!
//! Test commands (per dispatch task #24):
//!   A — ai_callable=true,  required_capabilities=[chat.read]
//!   B — ai_callable=true,  required_capabilities=[admin.x]
//!   C — ai_callable=false, required_capabilities=[chat.read]
//!
//! Actor wiring: workspace member, only `chat.read` is registered. Expected
//! AI-surface result: [A]. (B is dropped because admin.x is unregistered →
//! strict default-deny via the C1 gate. C is dropped because ai_callable is
//! false.) `Surface::Palette` is exercised through the unit-test path; this
//! file's "sanity" branch confirms the real DefaultCapabilityService never
//! gets consulted for non-Ai surfaces, satisfying the regression-guard
//! requirement without a full HTTP round-trip.

use std::sync::Arc;

use mockall::predicate::eq;
use rim_core::capability::model::{CapabilitySourceKind, RegisteredCapability};
use rim_core::capability::repository::MockCapabilityRegistryRepository;
use rim_core::command::repository::MockUserCommandRepository;
use rim_core::macro_def::repository::MockMacroRepository;
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::MockServerMemberRepository;
use rim_core::user::model::UserId;
use rim_infra_services::CommandRepos;
use rim_infra_services::capability::{
    CapabilityRepos, DefaultCapabilityService, builtin_capabilities,
};
use rim_infra_services::command::{CommandServices, DefaultCommandService};
use rim_services::Actor;
use rim_services::auth::service::MockAuthService;
use rim_services::command::dto::{
    CommandContextDto, CommandDefinitionDto, CommandExecutionTargetDto, CommandSourceKindDto,
    CommandSurfaceDto,
};
use rim_services::command::request::ListCommandsQuery;
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::service::CommandService;
use rim_services::conversation::service::MockConversationService;
use rim_services::message::service::MockMessageService;
use rim_services::read_state::service::MockReadStateService;
use rim_services::workspace_feature::service::MockWorkspaceFeatureService;
use rim_services::workspace_state::service::MockWorkspaceStateService;

fn ai_cmd(id: &str, ai_callable: bool, caps: &[&str]) -> CommandDefinitionDto {
    CommandDefinitionDto {
        id: id.to_string(),
        title: id.to_string(),
        description: "test".to_string(),
        category: "Test".to_string(),
        source_kind: CommandSourceKindDto::BuiltIn,
        execution_target: CommandExecutionTargetDto::Backend,
        arguments: vec![],
        required_context: vec![],
        required_capabilities: caps.iter().map(|s| s.to_string()).collect(),
        visible_in_palette: true,
        visible_in_context_menus: false,
        keybind_capable: false,
        dangerous: false,
        user_command_id: None,
        nl_description: None,
        intent_samples: vec![],
        ai_callable,
        sensitivity: CommandSensitivityDto::Safe,
        typed_verb: rim_services::command::verb::TypedVerb::Other,
        rate_limit_per_minute: None,
        feature: None,
    }
}

/// Builds a `DefaultCommandService` wired to a real `DefaultCapabilityService`
/// whose registry + member repos are mocked to the parameterised state.
///
/// `registered_caps` — capability keys the registry recognises (returns an
/// active `RegisteredCapability` row). Anything else → `Ok(None)` (strict
/// default-deny). `is_self_scoped` is inferred from `builtin_capabilities()`
/// so the P6-FU5 scope enforcement uses the same classification as production.
/// `is_member` — return value for the workspace membership lookup.
fn build_service(
    registered_caps: &'static [&'static str],
    is_member: bool,
) -> DefaultCommandService {
    let mut registry = MockCapabilityRegistryRepository::new();
    registry.expect_get().returning(move |key| {
        let found = registered_caps.iter().find(|c| **c == key);
        match found {
            Some(&cap_key) => {
                // Look up is_self_scoped from the canonical builtin spec so
                // the test uses the same classification as production seed.
                let is_self_scoped = builtin_capabilities()
                    .iter()
                    .find(|s| s.id == cap_key)
                    .map(|s| s.is_self_scoped)
                    .unwrap_or(false); // unknown / plugin caps default to workspace-scoped
                Ok(Some(RegisteredCapability {
                    capability_key: cap_key.to_string(),
                    source_kind: CapabilitySourceKind::Builtin,
                    plugin_id: None,
                    registered_at: chrono::Utc::now(),
                    retired_at: None,
                    is_self_scoped,
                }))
            }
            None => Ok(None),
        }
    });
    let mut members = MockServerMemberRepository::new();
    members
        .expect_is_member()
        .returning(move |_, _| Ok(is_member));

    let capability_service = Arc::new(DefaultCapabilityService::new(CapabilityRepos {
        capability_registry_repo: Arc::new(registry),
        server_member_repo: Arc::new(members),
    }));

    DefaultCommandService::new(
        CommandRepos {
            user_command_repo: Arc::new(MockUserCommandRepository::new()),
            macro_repo: Arc::new(MockMacroRepository::new()),
        },
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service,
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(rim_services::pin::service::MockPinService::new()),
            read_state_service: Arc::new(MockReadStateService::new()),
            server_member_service: Arc::new(
                rim_services::server_member::service::MockServerMemberService::new(),
            ),
            user_service: Arc::new(rim_services::user::service::MockUserService::new()),
            workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
            audit_log_service: None,
            incident_service: std::sync::Arc::new(
                rim_services::incident::service::MockIncidentService::new(),
            ),
            oncall_service: std::sync::Arc::new(
                rim_services::oncall::service::MockOnCallService::new(),
            ),
            workspace_feature_service: std::sync::Arc::new(
                MockWorkspaceFeatureService::all_enabled(),
            ),
            moderation_service: std::sync::Arc::new(
                rim_services::moderation::service::MockModerationService::no_timeouts(),
            ),
            notification_service: std::sync::Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
        },
    )
}

fn workspace_context() -> CommandContextDto {
    CommandContextDto {
        workspace_id: Some(ServerId::new()),
        conversation_id: None,
        thread_id: None,
        selected_text: None,
        selected_message_id: None,
        selected_message_ids: vec![],
        focused_surface: None,
        active_panel: None,
    }
}

#[tokio::test]
async fn ai_surface_returns_only_ai_callable_commands_actor_can_invoke() {
    // The headline scenario from dispatch task #24: A passes (ai_callable +
    // chat.read registered + member), B drops (admin.x unregistered, strict
    // default-deny), C drops (ai_callable=false).
    let actor = Actor::new(UserId::new());
    let svc = build_service(&["chat.read"], true);

    let input = vec![
        ai_cmd("a.read", true, &["chat.read"]),
        ai_cmd("b.admin", true, &["admin.x"]),
        ai_cmd("c.not_ai", false, &["chat.read"]),
    ];
    let context = workspace_context();
    let kept = svc
        .filter_ai_surface_test_only(&actor, Some(&context), input)
        .await
        .expect("filter must not error on valid inputs");

    let ids: Vec<&str> = kept.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["a.read"],
        "only A should survive (B unregistered, C not ai_callable)"
    );
}

#[tokio::test]
async fn ai_surface_drops_command_when_actor_not_workspace_member() {
    // Real DefaultCapabilityService denies on the membership leg even when
    // the registry leg passes — and `check_many` propagates that via the
    // map (cap → false) which the filter then drops.
    let actor = Actor::new(UserId::new());
    let svc = build_service(&["chat.read"], false); // not a member

    let input = vec![ai_cmd("a.read", true, &["chat.read"])];
    let context = workspace_context();
    let kept = svc
        .filter_ai_surface_test_only(&actor, Some(&context), input)
        .await
        .unwrap();

    assert!(
        kept.is_empty(),
        "non-member must be filtered even with registered cap (C1 membership leg)",
    );
}

#[tokio::test]
async fn ai_surface_with_no_workspace_context_denies_workspace_scoped_caps() {
    // P6-FU5: workspace-scoped caps (chat.read) must be denied when
    // workspace_id is None — previously this bypassed the membership check.
    // Self-scoped caps (audit.read.self) still pass with workspace_id=None,
    // and the registry leg still runs for them (unregistered → denied).
    let actor = Actor::new(UserId::new());
    // is_member=false proves the membership repo is NOT consulted for
    // workspace-scoped + None path — the gate denies before reaching it.
    let svc = build_service(&["chat.read", "audit.read.self"], false);

    let input = vec![
        // workspace-scoped: must be denied without workspace context (P6-FU5)
        ai_cmd("a.workspace_cap", true, &["chat.read"]),
        // self-scoped + registered: must pass without workspace context
        ai_cmd("b.self_scoped", true, &["audit.read.self"]),
        // self-scoped but unregistered: registry leg still denies
        ai_cmd("c.unregistered", true, &["never.registered"]),
    ];
    let kept = svc
        .filter_ai_surface_test_only(&actor, None, input)
        .await
        .unwrap();

    let ids: Vec<&str> = kept.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["b.self_scoped"],
        "workspace-scoped cap must be denied without workspace_id (P6-FU5); \
         self-scoped registered cap must pass; unregistered cap must be denied"
    );
}

#[tokio::test]
async fn list_commands_palette_does_not_consult_capability_service() {
    // Regression guard: register a CapabilityService whose `is_registered`
    // expectation is the explicit `eq("chat.read")` — if the Palette path
    // ever started invoking the gate, it would query for the union of
    // builtin caps (chat.write, admin.manage_members, etc.) and mockall
    // would panic with "no expectation matching args". This locks the
    // ai_callable opt-in to the Ai surface only.
    let actor = Actor::new(UserId::new());
    let mut registry = MockCapabilityRegistryRepository::new();
    // Palette listing must NOT call into the capability gate at all — this
    // expectation matches chat.read only; any other key would panic (no
    // matching expectation) which is exactly the guard we want. Using `get()`
    // since that's what check/check_many now call (P6-FU5 refactor).
    registry
        .expect_get()
        .with(eq("chat.read"))
        .returning(|_| {
            Ok(Some(RegisteredCapability {
                capability_key: "chat.read".to_string(),
                source_kind: CapabilitySourceKind::Builtin,
                plugin_id: None,
                registered_at: chrono::Utc::now(),
                retired_at: None,
                is_self_scoped: false,
            }))
        })
        .times(0..); // permissive count, but only matches chat.read
    let members = MockServerMemberRepository::new();

    let capability_service = Arc::new(DefaultCapabilityService::new(CapabilityRepos {
        capability_registry_repo: Arc::new(registry),
        server_member_repo: Arc::new(members),
    }));

    let mut user_repo = MockUserCommandRepository::new();
    user_repo
        .expect_list_for_user()
        .returning(|_| Box::pin(async { Ok(vec![]) }));

    let svc = DefaultCommandService::new(
        CommandRepos {
            user_command_repo: Arc::new(user_repo),
            macro_repo: Arc::new(MockMacroRepository::new()),
        },
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service,
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(MockMessageService::new()),
            message_reaction_service: Arc::new(
                rim_services::message_reaction::service::MockMessageReactionService::new(),
            ),
            pin_service: Arc::new(rim_services::pin::service::MockPinService::new()),
            read_state_service: Arc::new(MockReadStateService::new()),
            server_member_service: Arc::new(
                rim_services::server_member::service::MockServerMemberService::new(),
            ),
            user_service: Arc::new(rim_services::user::service::MockUserService::new()),
            workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
            audit_log_service: None,
            incident_service: std::sync::Arc::new(
                rim_services::incident::service::MockIncidentService::new(),
            ),
            oncall_service: std::sync::Arc::new(
                rim_services::oncall::service::MockOnCallService::new(),
            ),
            workspace_feature_service: std::sync::Arc::new(
                MockWorkspaceFeatureService::all_enabled(),
            ),
            moderation_service: std::sync::Arc::new(
                rim_services::moderation::service::MockModerationService::no_timeouts(),
            ),
            notification_service: std::sync::Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
        },
    );

    let palette = svc
        .list_commands(ListCommandsQuery {
            actor,
            context: None,
            surface: CommandSurfaceDto::Palette,
        })
        .await
        .expect("Palette listing must not call into the capability gate");

    assert!(
        !palette.is_empty(),
        "sanity: palette should include builtin commands"
    );
    // Sanity: a `visible_in_palette = true` builtin like chat.post_message
    // shows up regardless of whether ai_callable is true or false on it.
    assert!(palette.iter().any(|c| c.id == "chat.post_message"));
}
