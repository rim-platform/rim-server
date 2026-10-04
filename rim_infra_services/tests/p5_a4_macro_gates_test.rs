//! P5-A4 integration tests — Skill Macros AI exposure gate coverage.
//!
//! These tests drive `DefaultMacroService` through its public `execute_macro`
//! entry point with mock repos and services. Nine scenarios mandated by the
//! Hawkeye BLOCK-6 finding:
//!
//! 1. **AI gate** — agentic initiator denied on a non-`ai_callable` macro.
//! 2. **Sensitivity max-rule** — `effective_sensitivity` raises the macro's
//!    declared Restricted tier; confirmation gate fires accordingly.
//! 3. **Effective sensitivity raises when step is restricted** — a Safe macro
//!    with a Restricted step must raise to Restricted.
//! 4. **Confirmation gate (no proof)** — Restricted macro returns
//!    `ConfirmationRequired` when no proof is supplied.
//! 5. **Garbage proof rejected** — a garbage token with a real store is
//!    rejected with `PermissionDenied`.
//! 6. **Confirmation gate (valid proof)** — Restricted macro with a real
//!    issued proof proceeds to execute.
//! 7. **Partial failure emits parent + per-step children** — when a step
//!    errors, the service emits one child row plus one parent row.
//! 8. **Failed-at-step index recorded** — `error_kind` on the parent row
//!    encodes `step_error_at_0` (or whichever index failed).
//! 9. **AI surface includes synthesized macro definitions** — `list_commands`
//!    with `surface=Ai` delegates to `MacroService::synthesize_command_definitions`.
//!
//! All tests use in-process mocks — no running Postgres required.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rim_core::macro_def::model::{MacroDef, MacroId};
use rim_core::server::model::ServerId;
use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
use rim_core::user::model::UserId;
use rim_infra_services::macro_def::{DefaultMacroService, MacroRepos, MacroServices};
use rim_services::Actor;
use rim_services::ServiceError;
use rim_services::audit::dto::{AuditLogEntryDto, AuditOutcome};
use rim_services::audit::service::MockAuditLogService;
use rim_services::command::confirmation::{
    ConfirmationStore, issue_in_band, shared_in_memory_store,
};
use rim_services::command::dto::{
    CommandDefinitionDto, CommandExecutionTargetDto, CommandResultDto, CommandResultStatusDto,
    CommandSourceKindDto,
};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::sensitivity::CommandSensitivityDto;
use rim_services::command::service::MockCommandService;
use rim_services::command::verb::TypedVerb;
use rim_services::macro_def::request::ExecuteMacroCmd;
use rim_services::macro_def::service::MacroService;

use async_trait::async_trait;
use rim_core::macro_def::repository::MacroRepository;
use rim_core::repository::error::RepoResult;
use rim_core::server_member::repository::ServerMemberRepository;

// ── Local mocks ──────────────────────────────────────────────────────────────

mockall::mock! {
    TestMacroRepository {}
    #[async_trait]
    impl MacroRepository for TestMacroRepository {
        async fn get_by_id(&self, id: MacroId) -> RepoResult<Option<MacroDef>>;
        async fn find_by_name(
            &self,
            server_id: ServerId,
            user_id: UserId,
            name: &str,
        ) -> RepoResult<Option<MacroDef>>;
        async fn list_for_user(
            &self,
            server_id: ServerId,
            user_id: UserId,
        ) -> RepoResult<Vec<MacroDef>>;
        async fn list_for_server(&self, server_id: ServerId) -> RepoResult<Vec<MacroDef>>;
        async fn count_for_user(
            &self,
            server_id: ServerId,
            user_id: UserId,
        ) -> RepoResult<u64>;
        async fn create(&self, new_macro: rim_core::macro_def::model::NewMacroDef) -> RepoResult<MacroDef>;
        async fn update(&self, id: MacroId, update: rim_core::macro_def::model::UpdateMacroDef) -> RepoResult<MacroDef>;
        async fn delete(&self, id: MacroId) -> RepoResult<()>;
    }
}

mockall::mock! {
    TestServerMemberRepository {}
    #[async_trait]
    impl ServerMemberRepository for TestServerMemberRepository {
        async fn get_by_id(&self, id: ServerMemberId) -> RepoResult<Option<ServerMember>>;
        async fn get_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<Option<ServerMember>>;
        async fn add_member(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<ServerMember>;
        async fn change_member_role(&self, server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> RepoResult<()>;
        async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()>;
        async fn get_members_for_server(&self, server_id: ServerId) -> RepoResult<Vec<ServerMember>>;
        async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
    }
}

// ── Shared helpers ────────────────────────────────────────────────────────────

fn make_macro_def(
    id: MacroId,
    user_id: UserId,
    server_id: ServerId,
    ai_callable: bool,
    sensitivity: &'static str,
    steps_json: serde_json::Value,
) -> MacroDef {
    MacroDef {
        id,
        server_id,
        user_id,
        name: "test-macro".to_string(),
        description: None,
        steps_json,
        ai_callable,
        nl_description: String::new(),
        intent_samples: vec![],
        sensitivity: sensitivity.to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

fn single_step_json() -> serde_json::Value {
    serde_json::json!([{ "commandId": "chat.post_message", "args": {"text": "hi"} }])
}

fn build_service(
    macro_repo: MockTestMacroRepository,
    command_service: MockCommandService,
    audit_log_service: Option<MockAuditLogService>,
) -> DefaultMacroService {
    build_service_with_store(macro_repo, command_service, audit_log_service, None)
}

fn build_service_with_store(
    macro_repo: MockTestMacroRepository,
    command_service: MockCommandService,
    audit_log_service: Option<MockAuditLogService>,
    confirmation_store: Option<Arc<dyn ConfirmationStore>>,
) -> DefaultMacroService {
    let mut member_repo = MockTestServerMemberRepository::new();
    member_repo.expect_is_member().returning(|_, _| Ok(true));

    DefaultMacroService::new(
        MacroRepos {
            macro_repo: Arc::new(macro_repo),
            server_member_repo: Arc::new(member_repo),
        },
        MacroServices {
            command_service: Arc::new(command_service),
            audit_log_service: audit_log_service
                .map(|s| Arc::new(s) as Arc<dyn rim_services::audit::service::AuditLogService>),
            confirmation_store,
        },
    )
}

fn stub_command_definition(sensitivity: CommandSensitivityDto) -> CommandDefinitionDto {
    CommandDefinitionDto {
        id: "chat.post_message".to_string(),
        title: "Post Message".to_string(),
        description: "Post a message".to_string(),
        category: "Chat".to_string(),
        source_kind: CommandSourceKindDto::BuiltIn,
        execution_target: CommandExecutionTargetDto::Backend,
        arguments: vec![],
        required_context: vec![],
        required_capabilities: vec![],
        visible_in_palette: true,
        visible_in_context_menus: false,
        keybind_capable: false,
        dangerous: false,
        user_command_id: None,
        nl_description: None,
        intent_samples: vec![],
        ai_callable: false,
        sensitivity,
        typed_verb: TypedVerb::Other,
        rate_limit_per_minute: None,
        feature: None,
    }
}

/// Like `stub_command_definition`, but for an arbitrary step `id` — used by the C2 §6.6 macro
/// rollup test where the step is `incident.ack`, not `chat.post_message`.
fn stub_command_definition_with_id(
    id: &str,
    sensitivity: CommandSensitivityDto,
) -> CommandDefinitionDto {
    CommandDefinitionDto {
        id: id.to_string(),
        ..stub_command_definition(sensitivity)
    }
}

// ── Test 1: AI gate — agentic initiator denied on non-ai_callable macro ──────

#[tokio::test]
async fn ai_gate_denies_agent_initiator_on_non_ai_callable_macro() {
    let user_id = UserId::new();
    let macro_id = MacroId::new();
    let server_id = ServerId::new();

    let mut macro_repo = MockTestMacroRepository::new();
    macro_repo.expect_get_by_id().return_once(move |_| {
        Ok(Some(make_macro_def(
            macro_id,
            user_id,
            server_id,
            false,
            "safe",
            single_step_json(),
        )))
    });

    let svc = build_service(macro_repo, MockCommandService::new(), None);

    let err = svc
        .execute_macro(ExecuteMacroCmd {
            actor: Actor::new(user_id),
            macro_id,
            command_context: None,
            confirmation: None,
            initiator: CommandInitiator::Agent,
        })
        .await
        .unwrap_err();

    assert!(
        matches!(err, ServiceError::PermissionDenied),
        "expected PermissionDenied from AI gate, got {err:?}"
    );
}

// ── Test 2: Sensitivity max-rule — Restricted declared tier triggers confirm ──

/// Macro declared `sensitivity = "restricted"` → confirmation gate fires before
/// any step runs. `MockCommandService` panics if any method is called, so this
/// test also verifies `get_command` is NOT called before the gate fires.
#[tokio::test]
async fn sensitivity_max_rule_restricted_macro_requires_confirmation() {
    let user_id = UserId::new();
    let macro_id = MacroId::new();
    let server_id = ServerId::new();

    let mut macro_repo = MockTestMacroRepository::new();
    macro_repo.expect_get_by_id().return_once(move |_| {
        Ok(Some(make_macro_def(
            macro_id,
            user_id,
            server_id,
            true,
            "restricted",
            single_step_json(),
        )))
    });

    // Sensitivity is resolved via get_command for each step; step itself
    // is not dispatched since effective tier triggers the confirmation gate.
    let mut command_service = MockCommandService::new();
    command_service
        .expect_get_command()
        .times(1)
        .returning(|_| Box::pin(async { Ok(None) }));

    let svc = build_service(macro_repo, command_service, None);

    let result = svc
        .execute_macro(ExecuteMacroCmd {
            actor: Actor::new(user_id),
            macro_id,
            command_context: None,
            confirmation: None,
            initiator: CommandInitiator::Palette,
        })
        .await
        .expect("execute_macro should return Ok(ConfirmationRequired), not Err");

    assert_eq!(
        result.status,
        CommandResultStatusDto::ConfirmationRequired,
        "Restricted-sensitivity macro without proof must return ConfirmationRequired"
    );
}

// ── Test 3: Step sensitivity raises effective tier ────────────────────────────

/// A macro declared Safe but containing a Restricted step must raise the
/// effective sensitivity to Restricted and fire the confirmation gate.
/// This covers the max-rule direction where the step, not the macro, is the
/// higher tier.
#[tokio::test]
async fn effective_sensitivity_raises_when_step_is_restricted() {
    let user_id = UserId::new();
    let macro_id = MacroId::new();
    let server_id = ServerId::new();

    let mut macro_repo = MockTestMacroRepository::new();
    macro_repo.expect_get_by_id().return_once(move |_| {
        Ok(Some(make_macro_def(
            macro_id,
            user_id,
            server_id,
            true,
            "safe", // macro itself is Safe
            single_step_json(),
        )))
    });

    // Step is Restricted → effective becomes Restricted.
    let mut command_service = MockCommandService::new();
    command_service
        .expect_get_command()
        .times(1)
        .returning(|_| {
            Box::pin(async {
                Ok(Some(stub_command_definition(
                    CommandSensitivityDto::Restricted,
                )))
            })
        });

    let svc = build_service(macro_repo, command_service, None);

    let result = svc
        .execute_macro(ExecuteMacroCmd {
            actor: Actor::new(user_id),
            macro_id,
            command_context: None,
            confirmation: None,
            initiator: CommandInitiator::Palette,
        })
        .await
        .expect("execute_macro should return Ok(ConfirmationRequired)");

    assert_eq!(
        result.status,
        CommandResultStatusDto::ConfirmationRequired,
        "Safe macro wrapping a Restricted step must escalate to ConfirmationRequired"
    );
}

// ── Test 4: Garbage proof rejected for Restricted macro ──────────────────────

/// When a real `ConfirmationStore` is wired, submitting a garbage
/// `InBandToken` must be rejected with `PermissionDenied`.
#[tokio::test]
async fn garbage_proof_rejected_for_restricted_macro() {
    use rim_services::command::confirmation::ConfirmationProof;

    let user_id = UserId::new();
    let macro_id = MacroId::new();
    let server_id = ServerId::new();

    let mut macro_repo = MockTestMacroRepository::new();
    macro_repo.expect_get_by_id().return_once(move |_| {
        Ok(Some(make_macro_def(
            macro_id,
            user_id,
            server_id,
            true,
            "restricted",
            single_step_json(),
        )))
    });

    let mut command_service = MockCommandService::new();
    command_service
        .expect_get_command()
        .times(1)
        .returning(|_| Box::pin(async { Ok(None) }));

    let store = shared_in_memory_store();
    let svc = build_service_with_store(macro_repo, command_service, None, Some(Arc::clone(&store)));

    let garbage_proof = ConfirmationProof::InBandToken {
        token: "definitely-not-a-real-token".to_string(),
        args_hash: "fake-hash".to_string(),
    };

    let err = svc
        .execute_macro(ExecuteMacroCmd {
            actor: Actor::new(user_id),
            macro_id,
            command_context: None,
            confirmation: Some(garbage_proof),
            initiator: CommandInitiator::Palette,
        })
        .await
        .unwrap_err();

    // UnknownToken from the store maps to ConfirmationStale (is_stale=true);
    // any rejection error is acceptable — the key invariant is that the
    // macro does NOT execute with a garbage proof.
    assert!(
        matches!(
            err,
            ServiceError::PermissionDenied | ServiceError::ConfirmationStale
        ),
        "garbage InBandToken must be rejected, got {err:?}"
    );
}

// ── Test 5: Valid issued proof allows restricted macro to execute ─────────────

/// The companion to Test 4: a proof issued via `issue_in_band` against the
/// same store passes verification and the step executes successfully.
#[tokio::test]
async fn confirmation_gate_allows_restricted_macro_with_valid_proof() {
    let user_id = UserId::new();
    let macro_id = MacroId::new();
    let server_id = ServerId::new();
    let actor = Actor::new(user_id);
    let macro_cmd_id = format!("macro.{macro_id}");

    let mut macro_repo = MockTestMacroRepository::new();
    macro_repo.expect_get_by_id().return_once(move |_| {
        Ok(Some(make_macro_def(
            macro_id,
            user_id,
            server_id,
            true,
            "restricted",
            single_step_json(),
        )))
    });

    let mut command_service = MockCommandService::new();
    command_service
        .expect_get_command()
        .times(1)
        .returning(|_| Box::pin(async { Ok(None) }));
    command_service
        .expect_execute_command()
        .times(1)
        .returning(|_| {
            Box::pin(async {
                Ok(CommandResultDto {
                    status: CommandResultStatusDto::Success,
                    message: "ok".to_string(),
                    payload: None,
                    follow_up_commands: vec![],
                    pipeline_stages: None,
                    confirmation_required: None,
                })
            })
        });

    let store = shared_in_memory_store();
    let challenge = issue_in_band(store.as_ref(), &actor, &macro_cmd_id, &None, &None);
    let proof = rim_services::command::confirmation::ConfirmationProof::InBandToken {
        token: challenge.token,
        args_hash: challenge.args_hash,
    };

    let svc = build_service_with_store(macro_repo, command_service, None, Some(Arc::clone(&store)));

    let result = svc
        .execute_macro(ExecuteMacroCmd {
            actor,
            macro_id,
            command_context: None,
            confirmation: Some(proof),
            initiator: CommandInitiator::Palette,
        })
        .await
        .expect("valid proof must allow execution");

    assert_eq!(
        result.status,
        CommandResultStatusDto::Success,
        "gate approved with valid proof — step must execute and return Success"
    );
}

// ── Test 6: Partial failure emits parent + per-step children ─────────────────

/// When a step returns an error, `execute_macro` must emit:
/// 1. One child audit row with `outcome = "step_error"` and `parent_audit_id` set.
/// 2. One parent audit row with `outcome = "macro_partial_failure"`.
/// Total: 2 `write_entry` calls.
#[tokio::test]
async fn partial_failure_emits_parent_plus_per_step_children() {
    let user_id = UserId::new();
    let macro_id = MacroId::new();
    let server_id = ServerId::new();

    let mut macro_repo = MockTestMacroRepository::new();
    macro_repo.expect_get_by_id().return_once(move |_| {
        Ok(Some(make_macro_def(
            macro_id,
            user_id,
            server_id,
            true,
            "safe",
            single_step_json(),
        )))
    });

    let mut command_service = MockCommandService::new();
    command_service
        .expect_get_command()
        .times(1)
        .returning(|_| Box::pin(async { Ok(None) }));
    command_service
        .expect_execute_command()
        .times(1)
        .returning(|_| Box::pin(async { Err(ServiceError::Internal(None)) }));

    let write_count = Arc::new(AtomicUsize::new(0));
    let write_count_clone = Arc::clone(&write_count);
    let child_ok = Arc::new(AtomicBool::new(false));
    let child_ok_clone = Arc::clone(&child_ok);
    let parent_ok = Arc::new(AtomicBool::new(false));
    let parent_ok_clone = Arc::clone(&parent_ok);

    let mut audit_service = MockAuditLogService::new();
    audit_service
        .expect_write_entry()
        .times(2)
        .returning(move |entry: AuditLogEntryDto| {
            write_count_clone.fetch_add(1, Ordering::SeqCst);
            if entry.parent_audit_id.is_some() && entry.outcome == AuditOutcome::StepError {
                child_ok_clone.store(true, Ordering::SeqCst);
            }
            if entry.parent_audit_id.is_none() && entry.outcome == AuditOutcome::MacroPartialFailure
            {
                parent_ok_clone.store(true, Ordering::SeqCst);
            }
            Box::pin(async { Ok(()) })
        });

    let svc = build_service(macro_repo, command_service, Some(audit_service));

    let result = svc
        .execute_macro(ExecuteMacroCmd {
            actor: Actor::new(user_id),
            macro_id,
            command_context: None,
            confirmation: None,
            initiator: CommandInitiator::Palette,
        })
        .await
        .expect("execute_macro must return Ok(Error-status), not a service-level Err");

    assert_eq!(result.status, CommandResultStatusDto::Error);
    assert_eq!(
        write_count.load(Ordering::SeqCst),
        2,
        "must emit 2 audit rows (child + parent)"
    );
    assert!(
        child_ok.load(Ordering::SeqCst),
        "child row must have parent_audit_id set and outcome=step_error"
    );
    assert!(
        parent_ok.load(Ordering::SeqCst),
        "parent row must have outcome=macro_partial_failure and no parent_audit_id"
    );
}

// ── Test 7: Failed-at-step index recorded in parent error_kind ───────────────

/// When step 0 fails, the parent audit row must carry
/// `error_kind = Some("step_error_at_0")`.
#[tokio::test]
async fn failed_at_step_index_recorded_in_audit() {
    let user_id = UserId::new();
    let macro_id = MacroId::new();
    let server_id = ServerId::new();

    let mut macro_repo = MockTestMacroRepository::new();
    macro_repo.expect_get_by_id().return_once(move |_| {
        Ok(Some(make_macro_def(
            macro_id,
            user_id,
            server_id,
            true,
            "safe",
            single_step_json(),
        )))
    });

    let mut command_service = MockCommandService::new();
    command_service
        .expect_get_command()
        .times(1)
        .returning(|_| Box::pin(async { Ok(None) }));
    command_service
        .expect_execute_command()
        .times(1)
        .returning(|_| Box::pin(async { Err(ServiceError::Internal(None)) }));

    let error_kind_recorded = Arc::new(AtomicBool::new(false));
    let error_kind_clone = Arc::clone(&error_kind_recorded);

    let mut audit_service = MockAuditLogService::new();
    audit_service
        .expect_write_entry()
        .times(2)
        .returning(move |entry: AuditLogEntryDto| {
            if entry.parent_audit_id.is_none()
                && entry.error_kind.as_deref() == Some("step_error_at_0")
            {
                error_kind_clone.store(true, Ordering::SeqCst);
            }
            Box::pin(async { Ok(()) })
        });

    let svc = build_service(macro_repo, command_service, Some(audit_service));

    svc.execute_macro(ExecuteMacroCmd {
        actor: Actor::new(user_id),
        macro_id,
        command_context: None,
        confirmation: None,
        initiator: CommandInitiator::Palette,
    })
    .await
    .expect("execute_macro must return Ok");

    assert!(
        error_kind_recorded.load(Ordering::SeqCst),
        "parent audit row must carry error_kind = \"step_error_at_0\""
    );
}

// ── Test 8: list_commands AI surface includes synthesized macro definitions ───

/// `DefaultMacroService::synthesize_command_definitions` must return a
/// `CommandDefinitionDto` for each `ai_callable = true` macro in the repo.
/// This test drives the macro service directly (not via command service) to
/// verify the single source of truth mapping.
#[tokio::test]
async fn list_commands_ai_surface_includes_synthesized_macro_definitions() {
    use rim_services::macro_def::request::SynthesizeMacroCommandsQuery;

    let user_id = UserId::new();
    let server_id = ServerId::new();
    let macro_id = MacroId::new();

    let mut macro_repo = MockTestMacroRepository::new();
    macro_repo.expect_list_for_user().return_once(move |_, _| {
        Ok(vec![
            MacroDef {
                id: macro_id,
                server_id,
                user_id,
                name: "my-macro".to_string(),
                description: Some("does stuff".to_string()),
                steps_json: single_step_json(),
                ai_callable: true,
                nl_description: "Post a greeting".to_string(),
                intent_samples: vec!["say hello".to_string()],
                sensitivity: "safe".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
            // Non-AI-callable macro — must NOT appear in synthesized list.
            MacroDef {
                id: MacroId::new(),
                server_id,
                user_id,
                name: "private-macro".to_string(),
                description: None,
                steps_json: single_step_json(),
                ai_callable: false,
                nl_description: String::new(),
                intent_samples: vec![],
                sensitivity: "safe".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        ])
    });

    let svc = build_service(macro_repo, MockCommandService::new(), None);

    let definitions = svc
        .synthesize_command_definitions(SynthesizeMacroCommandsQuery {
            actor: Actor::new(user_id),
            server_id,
        })
        .await
        .expect("synthesize_command_definitions must succeed");

    assert_eq!(
        definitions.len(),
        1,
        "only the ai_callable macro must be synthesized; got {definitions:?}"
    );
    let def = &definitions[0];
    assert!(
        def.id.contains(&macro_id.to_string()),
        "synthesized definition id must embed macro_id; got {}",
        def.id
    );
    assert!(
        def.ai_callable,
        "synthesized definition must have ai_callable = true"
    );
    assert_eq!(
        def.nl_description.as_deref(),
        Some("Post a greeting"),
        "nl_description must round-trip from macro"
    );
}

// ── P6-H1 regression tests ────────────────────────────────────────────────────

/// Bug 1 regression: `ConfirmationRequired` must carry a non-None
/// `confirmation_required` so the client actually gets a token to re-submit.
/// Previously, the challenge was `None` so the client had no proof to present.
#[tokio::test]
async fn confirmation_required_carries_non_none_challenge_token() {
    let user_id = UserId::new();
    let macro_id = MacroId::new();
    let server_id = ServerId::new();

    let mut macro_repo = MockTestMacroRepository::new();
    macro_repo.expect_get_by_id().return_once(move |_| {
        Ok(Some(make_macro_def(
            macro_id,
            user_id,
            server_id,
            true,
            "restricted",
            single_step_json(),
        )))
    });

    let mut command_service = MockCommandService::new();
    command_service
        .expect_get_command()
        .times(1)
        .returning(|_| Box::pin(async { Ok(None) }));

    let store = shared_in_memory_store();
    let svc = build_service_with_store(macro_repo, command_service, None, Some(Arc::clone(&store)));

    let result = svc
        .execute_macro(ExecuteMacroCmd {
            actor: Actor::new(user_id),
            macro_id,
            command_context: None,
            confirmation: None,
            initiator: CommandInitiator::Palette,
        })
        .await
        .expect("must return Ok(ConfirmationRequired), not Err");

    assert_eq!(result.status, CommandResultStatusDto::ConfirmationRequired);
    assert!(
        result.confirmation_required.is_some(),
        "confirmation_required must be Some(challenge) so the client can re-submit; was None"
    );
}

/// Bug 2 regression: a Restricted step inside a macro must execute when the
/// outer proof is valid. Previously, `confirmation: None` was passed to the
/// inner execute_command, causing the step's gate to return ConfirmationRequired
/// which was then silently swallowed as Ok — the step never ran.
///
/// This test wires a real store, issues a valid outer proof, and asserts the
/// step's execute_command is actually reached.
#[tokio::test]
async fn restricted_step_executes_when_outer_proof_is_valid() {
    let user_id = UserId::new();
    let macro_id = MacroId::new();
    let server_id = ServerId::new();
    let actor = Actor::new(user_id);
    let macro_cmd_id = format!("macro.{macro_id}");

    let mut macro_repo = MockTestMacroRepository::new();
    macro_repo.expect_get_by_id().return_once(move |_| {
        Ok(Some(make_macro_def(
            macro_id,
            user_id,
            server_id,
            true,
            "restricted",
            single_step_json(),
        )))
    });

    let mut command_service = MockCommandService::new();
    command_service
        .expect_get_command()
        .times(1)
        .returning(|_| {
            Box::pin(async {
                Ok(Some(stub_command_definition(
                    CommandSensitivityDto::Restricted,
                )))
            })
        });
    // expect_execute_command MUST be called — proves the step actually ran.
    command_service
        .expect_execute_command()
        .times(1)
        .returning(|_| {
            Box::pin(async {
                Ok(CommandResultDto {
                    status: CommandResultStatusDto::Success,
                    message: "ok".to_string(),
                    payload: None,
                    follow_up_commands: vec![],
                    pipeline_stages: None,
                    confirmation_required: None,
                })
            })
        });

    let store = shared_in_memory_store();
    let challenge = issue_in_band(store.as_ref(), &actor, &macro_cmd_id, &None, &None);
    let proof = rim_services::command::confirmation::ConfirmationProof::InBandToken {
        token: challenge.token,
        args_hash: challenge.args_hash,
    };

    let svc = build_service_with_store(macro_repo, command_service, None, Some(Arc::clone(&store)));

    let result = svc
        .execute_macro(ExecuteMacroCmd {
            actor,
            macro_id,
            command_context: None,
            confirmation: Some(proof),
            initiator: CommandInitiator::Palette,
        })
        .await
        .expect("macro with valid proof must succeed");

    assert_eq!(
        result.status,
        CommandResultStatusDto::Success,
        "Restricted step must execute when outer proof is valid"
    );
}

/// Bug 3 regression: if a step's inner execute_command returns ConfirmationRequired
/// (e.g. confirmation: None passed, inner gate fires), it must be treated as
/// MacroPartialFailure — not silently swallowed as Ok.
#[tokio::test]
async fn inner_confirmation_required_from_step_is_partial_failure() {
    let user_id = UserId::new();
    let macro_id = MacroId::new();
    let server_id = ServerId::new();

    let mut macro_repo = MockTestMacroRepository::new();
    macro_repo.expect_get_by_id().return_once(move |_| {
        Ok(Some(make_macro_def(
            macro_id,
            user_id,
            server_id,
            true,
            "safe", // macro itself is Safe — no outer confirmation required
            single_step_json(),
        )))
    });

    let mut command_service = MockCommandService::new();
    command_service
        .expect_get_command()
        .times(1)
        .returning(|_| Box::pin(async { Ok(None) }));
    // Step returns ConfirmationRequired (simulates a dangerous step whose inner
    // gate fires because no per-step token was issued).
    command_service
        .expect_execute_command()
        .times(1)
        .returning(|_| {
            Box::pin(async {
                Ok(CommandResultDto {
                    status: CommandResultStatusDto::ConfirmationRequired,
                    message: "step requires confirmation".to_string(),
                    payload: None,
                    follow_up_commands: vec![],
                    pipeline_stages: None,
                    confirmation_required: None,
                })
            })
        });

    let svc = build_service(macro_repo, command_service, None);

    let result = svc
        .execute_macro(ExecuteMacroCmd {
            actor: Actor::new(user_id),
            macro_id,
            command_context: None,
            confirmation: None,
            initiator: CommandInitiator::Palette,
        })
        .await
        .expect("execute_macro must return Ok, not Err");

    assert_eq!(
        result.status,
        CommandResultStatusDto::Error,
        "inner ConfirmationRequired must surface as MacroPartialFailure (Error), not Success"
    );
}

// ── C2 §6.6: macro rollup — C2 commands compose into agent runbooks ─────────

/// An `ai_callable` macro containing an `incident.ack` step, executed by an `Agent` initiator,
/// requires per-step confirmation — pinning that C2's `ai_callable` flip (spec §1) composes
/// into agent-run macros under the *existing* rollup mechanism (no new rollup code needed).
///
/// The macro's declared tier is `Safe`; `incident.ack`'s tier is `Mutating`, which does not
/// cross the macro-level `requires_confirmation_by_tier` outer gate (Restricted-only, BLOCK 4 /
/// `macro_def.rs`), so no outer proof is verified and no per-step token is minted. The mocked
/// `execute_command` simulates exactly what the real dispatcher's confirmation gate
/// (`requires_confirmation_for_initiator`) returns in that situation for a Mutating command run
/// by an agentic initiator with no proof: `ConfirmationRequired`. Per Bug 3 (test above), that
/// inner `ConfirmationRequired` is a hard per-step failure — this is the same "step never
/// silently executes without agent confirmation" contract §1 relies on.
#[tokio::test]
async fn c2_ai_callable_macro_with_incident_ack_step_requires_confirmation_for_agent_initiator() {
    let user_id = UserId::new();
    let macro_id = MacroId::new();
    let server_id = ServerId::new();

    let step_json = serde_json::json!([
        { "commandId": "incident.ack", "args": {"incident_id": "11111111-1111-1111-1111-111111111111"} }
    ]);

    let mut macro_repo = MockTestMacroRepository::new();
    macro_repo.expect_get_by_id().return_once(move |_| {
        Ok(Some(make_macro_def(
            macro_id, user_id, server_id,
            true, // ai_callable — legal for an agentic initiator
            "safe", step_json,
        )))
    });

    let mut command_service = MockCommandService::new();
    command_service
        .expect_get_command()
        .times(1)
        .returning(|_| {
            Box::pin(async {
                Ok(Some(stub_command_definition_with_id(
                    "incident.ack",
                    CommandSensitivityDto::Mutating,
                )))
            })
        });
    command_service
        .expect_execute_command()
        .times(1)
        .withf(|cmd| cmd.command_id == "incident.ack" && cmd.initiator == CommandInitiator::Agent)
        .returning(|_| {
            Box::pin(async {
                Ok(CommandResultDto {
                    status: CommandResultStatusDto::ConfirmationRequired,
                    message: "confirmation required for dangerous command".to_string(),
                    payload: None,
                    follow_up_commands: vec![],
                    pipeline_stages: None,
                    confirmation_required: None,
                })
            })
        });

    let svc = build_service(macro_repo, command_service, None);

    let result = svc
        .execute_macro(ExecuteMacroCmd {
            actor: Actor::new(user_id),
            macro_id,
            command_context: None,
            confirmation: None,
            initiator: CommandInitiator::Agent,
        })
        .await
        .expect("execute_macro must return Ok, not Err");

    assert_eq!(
        result.status,
        CommandResultStatusDto::Error,
        "an Agent-run macro composing a C2 Mutating step (incident.ack) must require per-step \
         confirmation under the existing rollup — no new rollup code"
    );
}
