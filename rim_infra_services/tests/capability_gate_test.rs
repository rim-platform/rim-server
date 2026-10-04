//! C1 regression test — Phase 1 §3.3 (Black Widow / dispatch plan §Task 6).
//!
//! Mandatory CI gate: if `execute_command` returns `Ok` for a command whose
//! `required_capabilities` is non-empty when invoked through the public path
//! with an actor that lacks (or has-but-key-isn't-registered) that
//! capability, the strict default-deny gate has regressed and this test
//! MUST fail the build.
//!
//! The test wires a full `DefaultCommandService` and exercises the public
//! `execute_command` entry point — not a unit shortcut around the gate.
//! That distinction is deliberate: a future refactor that moves the gate
//! lower in the stack (or accidentally bypasses it for "trusted" call
//! paths) would only be caught here.

use std::sync::Arc;

use rim_core::user::model::UserId;
use rim_infra_services::command::{CommandServices, DefaultCommandService};
use rim_services::Actor;
use rim_services::ServiceError;
use rim_services::auth::service::MockAuthService;
use rim_services::capability::service::MockCapabilityService;
use rim_services::command::dto::CommandContextDto;
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::request::ExecuteCommandCmd;
use rim_services::command::service::CommandService;
use rim_services::conversation::service::MockConversationService;
use rim_services::message::service::MockMessageService;
use rim_services::read_state::service::MockReadStateService;
use rim_services::workspace_feature::service::MockWorkspaceFeatureService;
use rim_services::workspace_state::service::MockWorkspaceStateService;

mod fixtures {
    use super::*;
    use rim_core::command::repository::MockUserCommandRepository;
    use rim_core::macro_def::repository::MockMacroRepository;
    use rim_infra_services::CommandRepos;

    pub fn build_service(capability_service: MockCapabilityService) -> DefaultCommandService {
        DefaultCommandService::new(
            CommandRepos {
                user_command_repo: Arc::new(MockUserCommandRepository::new()),
                macro_repo: Arc::new(MockMacroRepository::new()),
            },
            CommandServices {
                auth_service: Arc::new(MockAuthService::new()),
                capability_service: Arc::new(capability_service),
                conversation_service: Arc::new(MockConversationService::new()),
                // Crucially: no `expect_send_message`. If the gate fails
                // open, the dispatch path will reach this mock and panic
                // with "no expectation set" — surfacing the regression
                // through the test failure rather than silently sending a
                // message during the test run.
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
            },
        )
    }
}

fn actor() -> Actor {
    Actor::new(UserId::new())
}

/// **C1 closure — the canonical regression test.**
///
/// `chat.post_message` declares `required_capabilities = ["chat.write"]`.
/// We wire a CapabilityService that explicitly denies `chat.write`
/// (simulating either: actor not a workspace member, OR the cap not in the
/// registry, OR a future plugin-grant denial). The gate must turn the
/// denial into `ServiceError::PermissionDenied` AND prevent the message
/// from being sent.
#[tokio::test]
async fn execute_command_denies_when_capability_check_rejects() {
    let mut capability_service = MockCapabilityService::new();
    capability_service
        .expect_check()
        .withf(|_actor, _ws, cap| cap == "chat.write")
        .returning(|_, _, _| Err(ServiceError::PermissionDenied));

    let svc = fixtures::build_service(capability_service);

    let result = svc
        .execute_command(ExecuteCommandCmd {
            actor: actor(),
            command_id: "chat.post_message".to_string(),
            args: Some(serde_json::json!({
                "server_id": rim_core::server::model::ServerId::new().to_string(),
                "conversation_id": rim_core::conversation::model::ConversationId::new().to_string(),
                "text": "should never be sent"
            })),
            context: Some(CommandContextDto {
                workspace_id: Some(rim_core::server::model::ServerId::new()),
                conversation_id: None,
                thread_id: None,
                selected_text: None,
                selected_message_id: None,
                selected_message_ids: vec![],
                focused_surface: None,
                active_panel: None,
            }),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Palette,
        })
        .await;

    match result {
        Err(ServiceError::PermissionDenied) => {}
        Err(other) => {
            panic!("C1 regression: expected PermissionDenied from capability gate, got {other:?}")
        }
        Ok(_) => panic!(
            "C1 regression: capability gate failed open — execute_command returned Ok despite \
             CapabilityService::check returning PermissionDenied"
        ),
    }
}

/// Companion test: when the gate APPROVES, the dispatch path proceeds.
/// Without this, a buggy refactor that always denies would still pass the
/// regression above. Pairing the two locks the gate's semantics from both
/// directions.
#[tokio::test]
async fn execute_command_proceeds_when_capability_check_passes() {
    let mut capability_service = MockCapabilityService::new();
    capability_service
        .expect_check()
        .withf(|_actor, _ws, cap| cap == "chat.write")
        .returning(|_, _, _| Ok(()));

    // Now we DO need send_message to be wired — the dispatch should reach
    // it. Build a fresh service with an explicit message-service mock.
    use rim_core::command::repository::MockUserCommandRepository;
    use rim_core::macro_def::repository::MockMacroRepository;
    use rim_infra_services::CommandRepos;
    use rim_services::message::dto::MessageDto;

    let mut message_service = MockMessageService::new();
    message_service
        .expect_send_message()
        .return_once(move |cmd| {
            Box::pin(async move {
                Ok(MessageDto {
                    id: rim_core::message::model::MessageId::new(),
                    server_id: cmd.server_id,
                    conversation_id: cmd.conversation_id,
                    thread_id: None,
                    root_thread_id: None,
                    author_id: cmd.actor.user_id,
                    text: cmd.text,
                    has_attachments: false,
                    attachments: vec![],
                    parent_message_id: None,
                    forwarded_from_message_id: None,
                    thread_root_message_id: None,
                    is_deleted: false,
                    sent_at: chrono::Utc::now(),
                })
            })
        });

    let svc = DefaultCommandService::new(
        CommandRepos {
            user_command_repo: Arc::new(MockUserCommandRepository::new()),
            macro_repo: Arc::new(MockMacroRepository::new()),
        },
        CommandServices {
            auth_service: Arc::new(MockAuthService::new()),
            capability_service: Arc::new(capability_service),
            conversation_service: Arc::new(MockConversationService::new()),
            message_service: Arc::new(message_service),
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
        },
    );

    svc.execute_command(ExecuteCommandCmd {
        actor: actor(),
        command_id: "chat.post_message".to_string(),
        args: Some(serde_json::json!({
            "server_id": rim_core::server::model::ServerId::new().to_string(),
            "conversation_id": rim_core::conversation::model::ConversationId::new().to_string(),
            "text": "permitted"
        })),
        context: None,
        confirmation: None,
        is_pipe: false,
        initiator: CommandInitiator::Palette,
    })
    .await
    .expect("gate approval should let the dispatch reach the inner service");
}
