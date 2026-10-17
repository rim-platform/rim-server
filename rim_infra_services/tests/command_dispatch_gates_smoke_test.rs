//! End-to-end smoke test of the command dispatch gates.
//!
//! Single integration test that exercises every dispatch gate in a
//! single dispatch path:
//!
//! 1. **Capability gate** — denied actor never reaches confirmation.
//! 2. **Confirmation challenge** — first call on a dangerous
//!    command returns `ConfirmationRequired` with a fresh challenge.
//! 3. **Confirmation verify** — second call with a matching
//!    `InBandToken` proof passes verify and dispatches to the executor.
//! 4. **Stale-proof rejection** — submitting an expired token surfaces
//!    `ServiceError::ConfirmationStale` (caller-recoverable).
//! 5. **Initiator round-trip** — `CommandInitiator` is set on
//!    every call so the audit entry records the right provenance.
//!
//! The test uses the existing `chat.delete_message` builtin, which has the
//! exact shape the gates care about:
//!
//! - `dangerous: true` (triggers the confirmation gate)
//! - `required_capabilities: ["chat.write"]` (triggers the capability gate)
//! - `sensitivity: Restricted` (exercises the sensitivity metadata)
//! - executes via `MessageService::delete_message` (mockable, no DB)
//!
//! ## Why integration-level
//!
//! The unit-level pieces are already covered in their own modules
//! (`confirmation::tests`, `capability_gate_test`). What's NOT tested
//! anywhere else is the *interaction* between them — specifically that the
//! confirmation gate and capability gate compose correctly under the
//! dispatch ordering (cap → confirmation → execute), and that a real
//! challenge issued by the service round-trips cleanly through a real proof
//! on a later call. Moving the cap gate inside the confirmation block
//! would silently break the anti-probe ordering — this test catches it.

use std::sync::Arc;

use chrono::Duration;

use rim_core::command::repository::MockUserCommandRepository;
use rim_core::conversation::model::ConversationId;
use rim_core::macro_def::repository::MockMacroRepository;
use rim_core::message::model::MessageId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_infra_services::CommandRepos;
use rim_infra_services::command::{CommandServices, DefaultCommandService};
use rim_services::Actor;
use rim_services::ServiceError;
use rim_services::audit::dto::{AuditLogEntryDto, AuditOutcome};
use rim_services::audit::service::MockAuditLogService;
use rim_services::auth::service::MockAuthService;
use rim_services::capability::service::MockCapabilityService;
use rim_services::command::confirmation::{
    ConfirmationProof, InMemoryConfirmationStore, issue_in_band_with_ttl,
};
use rim_services::command::dto::{CommandContextDto, CommandResultStatusDto};
use rim_services::command::initiator::CommandInitiator;
use rim_services::command::request::ExecuteCommandCmd;
use rim_services::command::service::CommandService;
use rim_services::conversation::service::MockConversationService;
use rim_services::message::service::MockMessageService;
use rim_services::read_state::service::MockReadStateService;
use rim_services::workspace_state::service::MockWorkspaceStateService;

// ---------------------------------------------------------------------------
// Fixture builders
// ---------------------------------------------------------------------------

struct Fixture {
    workspace_id: ServerId,
    conversation_id: ConversationId,
    message_id: MessageId,
    actor: Actor,
}

impl Fixture {
    fn new() -> Self {
        Self {
            workspace_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            message_id: MessageId::new(),
            actor: Actor::new(UserId::new()),
        }
    }

    fn context(&self) -> Option<CommandContextDto> {
        Some(CommandContextDto {
            workspace_id: Some(self.workspace_id),
            conversation_id: Some(self.conversation_id),
            thread_id: None,
            selected_text: None,
            selected_message_id: Some(self.message_id),
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        })
    }

    fn execute_cmd(
        &self,
        confirmation: Option<ConfirmationProof>,
        initiator: CommandInitiator,
    ) -> ExecuteCommandCmd {
        ExecuteCommandCmd {
            actor: self.actor,
            command_id: "chat.delete_message".to_string(),
            args: None,
            context: self.context(),
            confirmation,
            is_pipe: false,
            initiator,
        }
    }
}

/// Builds a `DefaultCommandService` with:
/// - the given capability-service mock (caller seeds expectations),
/// - the given message-service mock (caller seeds `expect_delete_message` iff
///   the test exercises a path that actually dispatches),
/// - a *shared* `InMemoryConfirmationStore` (returned alongside the service
///   as a typed handle) so tests can both pre-seed tokens via
///   `issue_in_band_with_ttl` AND introspect `len()` for token leaks
///   (scenario 1).
fn build_service(
    capability_service: MockCapabilityService,
    message_service: MockMessageService,
) -> (DefaultCommandService, Arc<InMemoryConfirmationStore>) {
    let store = Arc::new(InMemoryConfirmationStore::new());
    let svc = DefaultCommandService::with_confirmation_store(
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
            moderation_service: std::sync::Arc::new(
                rim_services::moderation::service::MockModerationService::no_timeouts(),
            ),
            notification_service: std::sync::Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
        },
        Arc::clone(&store) as Arc<dyn rim_services::command::confirmation::ConfirmationStore>,
    );
    (svc, store)
}

fn build_service_with_audit(
    capability_service: MockCapabilityService,
    message_service: MockMessageService,
    audit_service: MockAuditLogService,
) -> (DefaultCommandService, Arc<InMemoryConfirmationStore>) {
    let store = Arc::new(InMemoryConfirmationStore::new());
    let svc = DefaultCommandService::with_confirmation_store(
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
            audit_log_service: Some(
                Arc::new(audit_service) as Arc<dyn rim_services::audit::service::AuditLogService>
            ),
            moderation_service: std::sync::Arc::new(
                rim_services::moderation::service::MockModerationService::no_timeouts(),
            ),
            notification_service: std::sync::Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
        },
        Arc::clone(&store) as Arc<dyn rim_services::command::confirmation::ConfirmationStore>,
    );
    (svc, store)
}

fn cap_grants_chat_write() -> MockCapabilityService {
    let mut cap = MockCapabilityService::new();
    cap.expect_check()
        .withf(|_actor, _ws, key| key == "chat.write")
        .returning(|_, _, _| Ok(()));
    cap
}

fn cap_denies_chat_write() -> MockCapabilityService {
    let mut cap = MockCapabilityService::new();
    cap.expect_check()
        .withf(|_actor, _ws, key| key == "chat.write")
        .returning(|_, _, _| Err(ServiceError::PermissionDenied));
    cap
}

// ---------------------------------------------------------------------------
// The smoke test — all five primitives in one path
// ---------------------------------------------------------------------------

/// End-to-end: cap-deny → cap-grant+no-proof → cap-grant+proof →
/// cap-grant+stale-proof → TOCTOU-args-mismatch → audit-entry-with-initiator.
/// All six scenarios against `chat.delete_message`, exercising the live dispatcher.
#[tokio::test]
async fn dispatch_gates_end_to_end_smoke_test() {
    let fx = Fixture::new();

    // ── Scenario 1 ────────────────────────────────────────────────────────
    // Capability gate denies. Two regressions to catch here:
    //
    // (a) Dispatch must NOT be reached. `MockMessageService` has NO
    //     `expect_delete_message`, so any reach panics in mockall teardown.
    //
    // (b) **No confirmation token may be issued.** A regression that swaps
    //     the gate ordering (confirmation BEFORE cap) would still surface
    //     PermissionDenied to the caller — masking the actual security
    //     leak: a token minted for the denied actor now sits in the store,
    //     and a later call could submit it. The `store.is_empty()`
    //     assertion below is the only way to catch this — observable
    //     behavior alone (return value) is insufficient.
    {
        let (svc, store) = build_service(cap_denies_chat_write(), MockMessageService::new());
        let result = svc
            .execute_command(fx.execute_cmd(None, CommandInitiator::Agent))
            .await;
        match result {
            Err(ServiceError::PermissionDenied) => {}
            other => {
                panic!("S1: expected PermissionDenied from cap gate, got {other:?}")
            }
        }
        assert!(
            store.is_empty(),
            "S1 token-leak regression: cap-denied actor must NOT have a confirmation token \
             issued for them — the cap gate MUST run before the confirmation gate"
        );
    }

    // ── Scenario 2 ────────────────────────────────────────────────────────
    // Capability passes. No proof submitted. Server must issue a fresh
    // challenge and return `ConfirmationRequired` — `delete_message` MUST
    // NOT be called (no expectation set; mock would panic if invoked). The
    // challenge shape (non-empty token + 64-char hex hash) is asserted too
    // so a regression that returns an empty token slips here, not silently
    // through to S3.
    {
        let (svc, store) = build_service(cap_grants_chat_write(), MockMessageService::new());
        let result = svc
            .execute_command(fx.execute_cmd(None, CommandInitiator::Agent))
            .await
            .expect("S2: cap passes + no proof should yield Ok(ConfirmationRequired)");
        assert_eq!(result.status, CommandResultStatusDto::ConfirmationRequired);
        let challenge = result
            .confirmation_required
            .expect("S2: ConfirmationRequired result must carry a challenge");
        assert!(
            !challenge.token.is_empty(),
            "S2 challenge token must be non-empty"
        );
        assert_eq!(
            challenge.args_hash.len(),
            64,
            "S2 args_hash must be a 64-char SHA-256 hex string (got {} chars)",
            challenge.args_hash.len()
        );
        assert_eq!(
            store.len(),
            1,
            "S2 must persist exactly one outstanding challenge in the store"
        );
    }

    // ── Scenario 3 ────────────────────────────────────────────────────────
    // Cap passes. Caller submits a *valid* proof. Dispatch reaches
    // `delete_message`. The challenge from S2 was issued by a DIFFERENT
    // service instance, so its store is gone and this scenario re-issues it. The
    // realistic flow is: (a) first call to a single service issues the
    // challenge, (b) second call to the *same* service consumes it. This
    // scenario models that with one service + two sequential calls.
    {
        let mut message_service = MockMessageService::new();
        let expected_msg_id = fx.message_id;
        let expected_ws = fx.workspace_id;
        let expected_conv = fx.conversation_id;
        message_service
            .expect_delete_message()
            .withf(move |cmd| {
                cmd.message_id == expected_msg_id
                    && cmd.server_id == expected_ws
                    && cmd.conversation_id == expected_conv
                    && !cmd.hard
            })
            .return_once(|_| Box::pin(async { Ok(()) }));

        let (svc, _store) = build_service(cap_grants_chat_write(), message_service);

        // (a) Issue challenge through the live dispatcher.
        let issue_result = svc
            .execute_command(fx.execute_cmd(None, CommandInitiator::Agent))
            .await
            .expect("S3a: should issue challenge");
        assert_eq!(
            issue_result.status,
            CommandResultStatusDto::ConfirmationRequired
        );
        let challenge = issue_result
            .confirmation_required
            .expect("S3a: must carry challenge");

        // (b) Re-submit with a matching proof. Same args / context / actor.
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: challenge.args_hash,
        };
        let exec_result = svc
            .execute_command(fx.execute_cmd(Some(proof), CommandInitiator::Agent))
            .await
            .expect("S3b: valid proof must execute");
        assert_eq!(exec_result.status, CommandResultStatusDto::Success);
        // delete_message expectation is automatically asserted on Drop
        // (mockall panics in test teardown if the expectation never fires).
    }

    // ── Scenario 4 ────────────────────────────────────────────────────────
    // Cap passes. Caller submits an EXPIRED proof. Server must surface
    // `ServiceError::ConfirmationStale` (caller-recoverable; the modal
    // should re-issue and retry). MockMessageService has no expectation —
    // dispatch must NOT be reached.
    {
        let (svc, store) = build_service(cap_grants_chat_write(), MockMessageService::new());

        // Pre-seed an already-expired token directly in the store. The
        // 1ms TTL + 20ms sleep guarantees expiry; the dispatcher's
        // verify_in_band call will surface either Expired (if the lazy
        // sweep didn't fire) or UnknownToken (if it did) — both classify
        // as `is_stale() == true` and both map to `ConfirmationStale` in
        // execute_command.
        let challenge = issue_in_band_with_ttl(
            store.as_ref(),
            &fx.actor,
            "chat.delete_message",
            &None,
            &fx.context(),
            Duration::milliseconds(1),
        );
        std::thread::sleep(std::time::Duration::from_millis(20));

        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: challenge.args_hash,
        };
        let result = svc
            .execute_command(fx.execute_cmd(Some(proof), CommandInitiator::Agent))
            .await;
        match result {
            Err(ServiceError::ConfirmationStale) => {}
            other => panic!("S4: expected ConfirmationStale for expired proof, got {other:?}"),
        }
    }

    // ── Scenario 5 ────────────────────────────────────────────────────────
    // TOCTOU defense. Caller obtains a valid challenge, then
    // submits the proof with a tampered `args_hash`. The service must reject
    // with `ConfirmationStale` (ArgsMismatch maps to is_stale → ConfirmationStale).
    //
    // This simulates the canonical TOCTOU attack: UI shows "delete message X",
    // user confirms, attacker swaps args to "delete message Y" before re-submit.
    // The server binds the proof to the args that were shown at challenge time;
    // any tampering is caught here.
    {
        let (svc, _store) = build_service(cap_grants_chat_write(), MockMessageService::new());

        // Issue challenge through the live dispatcher (same args/context as the rest of the fixture).
        let issue_result = svc
            .execute_command(fx.execute_cmd(None, CommandInitiator::Agent))
            .await
            .expect("S5a: should issue challenge");
        assert_eq!(
            issue_result.status,
            CommandResultStatusDto::ConfirmationRequired
        );
        let challenge = issue_result
            .confirmation_required
            .expect("S5a: must carry challenge");

        // Tamper the args_hash — flip every hex digit to 'f'. The proof token
        // is valid but the args_hash submitted no longer matches the stored one.
        let tampered_hash = "f".repeat(64);
        assert_ne!(
            tampered_hash, challenge.args_hash,
            "S5: tampered hash must differ from issued hash"
        );

        let tampered_proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: tampered_hash,
        };
        let result = svc
            .execute_command(fx.execute_cmd(Some(tampered_proof), CommandInitiator::Agent))
            .await;
        match result {
            Err(ServiceError::ConfirmationStale) => {}
            other => {
                panic!("S5: expected ConfirmationStale for TOCTOU args-mismatch, got {other:?}")
            }
        }
    }

    // ── Scenario 6 ────────────────────────────────────────────────────────
    // Audit entry integrity. After a successful dispatch
    // the audit log entry must record:
    // - `initiator` matching the CommandInitiator variant used ("agent")
    // - `sensitivity` matching the command definition ("restricted" for
    //   chat.delete_message, which is dangerous)
    // - `outcome` = Success
    //
    // This exercises the audit path end-to-end without mocking `write_entry`
    // behavior — the mock's `.returning()` captures the entry and records the
    // fields the test cares about. MockAuditLogService with `audit_log_service:
    // Some(...)` triggers the write path in execute_command.
    {
        use std::sync::atomic::{AtomicBool, Ordering};

        let initiator_ok = Arc::new(AtomicBool::new(false));
        let sensitivity_ok = Arc::new(AtomicBool::new(false));
        let outcome_ok = Arc::new(AtomicBool::new(false));

        let initiator_ok_c = Arc::clone(&initiator_ok);
        let sensitivity_ok_c = Arc::clone(&sensitivity_ok);
        let outcome_ok_c = Arc::clone(&outcome_ok);

        let mut audit_svc = MockAuditLogService::new();
        audit_svc
            .expect_write_entry()
            // The dispatcher emits one entry per execute_command call, so
            // exactly 2 are expected: one for the ConfirmationRequired challenge
            // call and one for the successful dispatch call.
            .returning(move |entry: AuditLogEntryDto| {
                if entry.outcome == AuditOutcome::Success {
                    // "agent" is the stable audit wire identifier for CommandInitiator::Agent.
                    if entry.initiator == "agent" {
                        initiator_ok_c.store(true, Ordering::SeqCst);
                    }
                    // chat.delete_message is dangerous → sensitivity = "restricted".
                    if entry.sensitivity == "restricted" {
                        sensitivity_ok_c.store(true, Ordering::SeqCst);
                    }
                    outcome_ok_c.store(true, Ordering::SeqCst);
                }
                Box::pin(async { Ok(()) })
            });

        let mut message_service = MockMessageService::new();
        let expected_msg_id = fx.message_id;
        let expected_ws = fx.workspace_id;
        let expected_conv = fx.conversation_id;
        message_service
            .expect_delete_message()
            .withf(move |cmd| {
                cmd.message_id == expected_msg_id
                    && cmd.server_id == expected_ws
                    && cmd.conversation_id == expected_conv
            })
            .return_once(|_| Box::pin(async { Ok(()) }));

        let (svc, _store) =
            build_service_with_audit(cap_grants_chat_write(), message_service, audit_svc);

        // (a) Issue the challenge — triggers ConfirmationRequired audit entry.
        let issue_result = svc
            .execute_command(fx.execute_cmd(None, CommandInitiator::Agent))
            .await
            .expect("S6a: should issue challenge");
        let challenge = issue_result
            .confirmation_required
            .expect("S6a: must carry challenge");

        // (b) Re-submit with valid proof — triggers Success audit entry.
        let proof = ConfirmationProof::InBandToken {
            token: challenge.token,
            args_hash: challenge.args_hash,
        };
        let exec_result = svc
            .execute_command(fx.execute_cmd(Some(proof), CommandInitiator::Agent))
            .await
            .expect("S6b: valid proof must execute");
        assert_eq!(exec_result.status, CommandResultStatusDto::Success);

        assert!(
            initiator_ok.load(Ordering::SeqCst),
            "S6: audit entry initiator must be 'agent' on successful dispatch"
        );
        assert!(
            sensitivity_ok.load(Ordering::SeqCst),
            "S6: audit entry sensitivity must be 'restricted' for chat.delete_message"
        );
        assert!(
            outcome_ok.load(Ordering::SeqCst),
            "S6: audit entry outcome must be Success"
        );
    }
}
