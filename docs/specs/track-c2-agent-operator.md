# Track C2 — The Agent Operator (incident lifecycle × AI agent fusion)

_Spec by iron-man, 2026-07-13. Requested by nick-fury, ratified by Jakub (CEO).
Implements ROADMAP Track C2 (coupled to Track D), pre-designed in
`docs/specs/track-c1-oncall.md` §10.1 FLAG E ("C1 registers, C2 fuses").
Context: `docs/next-steps-2026-07.md` §"Track C2 — the moat",
`docs/research/mattermost-playbooks-teardown-2026-07-10.md`._

**What ships:** the four C1 lifecycle commands (`oncall.who`, `oncall.take_shift`,
`incident.escalate`, `incident.ack`) become AI-callable under the existing
agentic confirmation gate, and agent-driven incident actions leave a visible
transcript in the incident timeline. Everyone else's AI *suggests*; RIM's *acts*
under a human confirmation gate — and the incident timeline proves it.

This is a **field/behavior upgrade, not a re-architecture**. No schema change.
No SDL change. One PR.

## §0 Rulings (summary — details in the sections below)

| Open choice | Ruling | One-line rationale |
|---|---|---|
| `required_capabilities` | **Keep `[]` on all four** | Builtin caps have no per-user grant store — the check is registry + server-membership, which is byte-for-byte what the service methods' `ensure_server_member` already enforces; adding caps would only add a `context.workspace_id` precondition that breaks arg-only agent calls and the C1 paged-toast/keybind ack. |
| Transcript hook layer | **Inside the service methods**, initiator threaded via a new `Option<CommandInitiator>` field on the two Cmd structs | The executor layer has no `incident_phase_repo` access (only service `Arc`s), and the service is where every other timeline note is written; `None` from the GraphQL path makes double-write impossible by construction. |
| SDL change | **None — skip codegen** | Flipping `ai_callable`/`nl_description` changes `commandDiscover`/`listCommands` *output values* through existing fields; the timeline note reuses the existing `incident_phase` mechanism; the Cmd field is service-layer only. |
| `incident.ack` warning copy carrier | **`Command.description`** (existing GraphQL field), strengthened; consequence duplicated in `nl_description` for the model | `ConfirmationChallengeDto` (token/args_hash/expires_at, `confirmation.rs:94-98`; GraphQL mirror `types.rs:666-670`) has no message field, and the copy is static per-command — the definition, not the challenge, is the right carrier. Adding a challenge field would be an SDL change for zero flexibility gain. |
| Agent exemption from ack confirmation (C1 §10.1 open flag) | **No exemption** (Jakub ratified) | Ack cancels the escalation chain — the exact class of action the human gate exists for. |

## §1 The four registrations

Files: `rim_infra_services/src/command/registry/incident.rs`,
`rim_infra_services/src/command/registry/oncall.rs`.

Change ONLY the fields listed per command. Everything else — `sensitivity`,
`dangerous`, `typed_verb`, `arguments`, `keybind_capable`, `visible_in_palette`,
`rate_limit_per_minute`, `required_capabilities` (stays `vec![]`, §2) — is
**unchanged**. Do NOT downgrade any sensitivity. Model the registration style on
`chat.create_channel` (`registry/chat.rs:56-113`), the working precedent for an
AI-callable Mutating builtin.

All strings below pass `sanitize_command_ai_fields`
(`rim_services/src/command/sanitize.rs`, run over every def at registration,
`registry/mod.rs:26-29`): plain text, no LLM structural tags, no
instruction-override phrasings (note the sanitizer rejects the literal phrase
"you are now " — none of the copy below uses it), `nl_description` ≤ 1000 chars,
each intent sample ≤ 200 chars, ≤ `MAX_INTENT_SAMPLES_COUNT` (5) samples.
The existing `builtin_commands_sanitize_cleanly` test is the CI backstop.

### 1a. `oncall.who` (Safe, Read — no confirmation for any initiator)

```
ai_callable: true,
nl_description: Some(
    "Look up who is currently on call for an on-call schedule, including \
     when their shift ends and who is next. Use when the user asks who is \
     on call or who to page."
        .to_string(),
),
intent_samples: vec![
    "who's on call for payments".to_string(),
    "who is on call right now".to_string(),
    "who do I page for this".to_string(),
    "when does the current on-call shift end".to_string(),
],
```

### 1b. `oncall.take_shift` (Mutating, Write — agent confirmation via existing gate)

```
ai_callable: true,
nl_description: Some(
    "Create an on-call override that puts the requesting user on call for \
     an explicit time window. Requires RFC 3339 start and end timestamps. \
     Use when the user asks to take, cover, or swap onto a shift."
        .to_string(),
),
intent_samples: vec![
    "put me on call until 6pm".to_string(),
    "I'll take the on-call shift tonight".to_string(),
    "cover the shift from 2pm to 8pm for me".to_string(),
    "take over on-call for the next four hours".to_string(),
],
```

### 1c. `incident.escalate` (Mutating — agent confirmation via existing gate)

```
ai_callable: true,
nl_description: Some(
    "Immediately fire the incident's current escalation step instead of \
     waiting for the timer, paging the next responders in the chain. Use \
     when the user asks to escalate an incident now."
        .to_string(),
),
intent_samples: vec![
    "escalate this now".to_string(),
    "escalate the checkout incident".to_string(),
    "page the next level for this incident".to_string(),
    "skip the wait and escalate".to_string(),
],
```

### 1d. `incident.ack` (Mutating — the sensitive one)

Acking **cancels the escalation chain and stops paging on-call**. The agent may
call it; the human confirmation must communicate the consequence (§4).

```
description: "Acknowledge an incident. This cancels the escalation chain and \
              stops paging on-call for this incident."
    .to_string(),
ai_callable: true,
nl_description: Some(
    "Acknowledge an incident on behalf of the user. Acknowledging cancels \
     the escalation chain and stops paging on-call for this incident, so \
     only call it when the user clearly wants to take ownership and stop \
     the paging."
        .to_string(),
),
intent_samples: vec![
    "ack the checkout incident".to_string(),
    "acknowledge this incident".to_string(),
    "I've got this incident, stop paging".to_string(),
    "mark the incident as acknowledged".to_string(),
],
```

Note the `description` change (from "Acknowledge an incident, cancelling its
escalation chain.") — this is deliberate; it is the FE-facing warning copy (§4).
Update the stale `ai_callable: false` doc comments in both registry files
(C1 wrote "C2 flips this later" — that is now).

### Confirmation behavior — already correct, verify, don't build

`requires_confirmation_for_initiator` (`rim_services/src/command/dto.rs:228-246`)
is the single source of truth: `initiator.is_agentic() && sensitivity >= Mutating`
→ confirmation. So flipping `ai_callable` alone gives:

- `oncall.who` (Safe): one-shot for every initiator, including Agent.
- The three Mutating commands: agent gets `ConfirmationRequired` + a
  `ConfirmationChallengeDto` (issued at `dispatch.rs:475-512`), retries with an
  `InBandToken` proof; human initiators (Palette/Keybind/ContextMenu/Cli) stay
  one-shot — **no C1 surface changes behavior** (banner ack, paged-toast ack,
  keybind ack all remain single-action).

**No code change to the gate.** Any PR that touches
`requires_confirmation_for_initiator` is out of spec.

Macro rollup also already works — `CommandSensitivityDto::effective()`
(`sensitivity.rs:148-153`) and the per-step token issuance + non-ai_callable
gating in `macro_def.rs:305-460`. Do NOT rebuild it; flipping `ai_callable`
makes these four legal steps in agent-routed macros for free (one test in §6.5
pins this).

## §2 `required_capabilities` — ruling: keep `[]` (all four)

**Ruling with justification (code-verified):**

1. `DefaultCapabilityService::check` (`rim_infra_services/src/capability.rs:147-220`)
   for a builtin capability is: key registered in `capability_registry` +
   `server_member_repo.is_member(workspace_id, actor.user_id)`. There is **no
   per-user/per-agent grant store for builtin caps** (that's a Phase-5/plugin
   construct). Seeding e.g. `incident.operate` in `builtin_capabilities()`
   (`capability.rs:44-89`) would auto-"grant" it to every server member — it is
   not a conscious user grant and not default-deny in any sense the service
   layer doesn't already provide: `acknowledge_incident` / `escalate_incident_now`
   / `who_is_on_call` / `create_override` all enforce `ensure_server_member`
   (verified: `incident.rs:1232-1237`, `incident.rs:1292-1297`; oncall service
   follows the same pattern). The cap check would be an exact duplicate.
2. Workspace-scoped caps **reject `workspace_id: None`** (`capability.rs:192-202`).
   These four commands take their target from args (`incident_id`/`schedule_id`),
   not context — an agent tool-call or a paged-toast/keybind ack without
   `context.workspace_id` would suddenly get `PermissionDenied`. That's a real
   regression risk on shipped C1 surfaces for zero security gain (see 1).
3. The agent runs as the user's `Actor` (client-side engine → `executeCommand`
   with `initiator: Agent`), so actor-level caps can never distinguish
   agent-vs-human anyway. The agent-specific boundary is the confirmation gate
   (initiator-aware), which Jakub ratified as mandatory for all three Mutating
   commands.
4. `filter_ai_surface` (`surface.rs:10-42`) with `[]` surfaces the four commands
   to any authenticated actor's tool list — acceptable: visibility is not
   authorization; execution is authz-gated at the service layer, and the
   inventory CI test (§5) + startup audit line keep the exposed set reviewed.

**Consequence:** no change to `builtin_capabilities()`, no seeding change, and
the moat demo needs no grant step. **Revisit** when a per-user capability grant
store exists (Track D3/plugin runbooks) — record that in `docs/oncall.md` (§8).

## §3 Incident-timeline transcript of agent-driven actions (the visible moat)

When an **agentic** initiator (`CommandInitiator::is_agentic()` — Agent, Plugin,
Mcp) executes `incident.ack` or `incident.escalate`, append an `incident_phase`
timeline note, reusing the exact mechanism C1 escalation-step firing uses
(`incident.rs:1353-1391`: `find_current_phase` → default `Detection` →
`incident_phase_repo.create(incident_id, phase, Some(note))`, best-effort with
`tracing::warn!` on failure — never fail the command over a note).

**Scope ruling:** `incident.ack` and `incident.escalate` only.
`oncall.who` is a pure read — no note. `oncall.take_shift` has no incident in
scope (schedule-scoped) — no note. (Agent-driven take_shift is still fully
audited via the command audit log, `command/mod.rs:334-359`.)

### 3a. Hook layer ruling: inside the service, initiator threaded via the Cmd

Why not the executor: `CommandServices` (executor's only handle) holds service
`Arc`s, not repos — the executor **cannot** reach `incident_phase_repo`. Why not
a second service method: the note must be atomic-ish with the action and share
its phase-resolution logic; and keeping it in the service means the GraphQL
path (`acknowledgeIncident`, `mutation.rs:1945`) shares one code path with the
initiator defaulted to `None` — a human mutation can never double-write, and an
agent action can never skip the note.

### 3b. Signature changes (all internal — public GraphQL API unchanged)

`rim_services/src/incident/command.rs` (`:65-76`) — add to BOTH structs, with a
doc comment:

```rust
pub struct AcknowledgeIncidentCmd {
    pub actor: Actor,
    pub incident_id: IncidentId,
    /// Command-dispatch initiator, when this call arrives via the command
    /// executor (C2 §3). `None` for direct GraphQL mutations. When
    /// `Some(i)` and `i.is_agentic()`, the service appends an
    /// agent-transcript timeline note to the incident.
    pub initiator: Option<rim_services::command::initiator::CommandInitiator>,
}
```

(same field + doc on `EscalateIncidentNowCmd`; `CommandInitiator` is `Copy`,
same crate — no import cycle.)

Thread the initiator down the dispatch chain (it is already in
`ExecutionContext`, `dispatch.rs:17-23`; it just gets dropped today):

- `DefaultCommandService::execute_builtin` (`dispatch.rs:193-201`): add
  `initiator: CommandInitiator` param; call site `dispatch.rs:434-436` passes
  `ctx.initiator`.
- `executor::execute_builtin_command` (`executor/mod.rs:23-29`): add
  `initiator: CommandInitiator` param. Only the `incident.ack` /
  `incident.escalate` match arms consume it; other arms ignore it (do NOT
  change the other domain executors' signatures).
- `executor/incident.rs` `ack`/`escalate`: accept `initiator` and construct
  `AcknowledgeIncidentCmd { actor, incident_id, initiator: Some(initiator) }`
  (resp. `EscalateIncidentNowCmd`).
- `rim_api/src/mutation.rs:1945` (`acknowledgeIncident` resolver): set
  `initiator: None`.
- Fix remaining struct-literal sites the compiler flags
  (`incident_unit_tests.rs:1546,1592` → `initiator: None`).

Note the macro path is automatically honest: nested/macro execution preserves
the outermost initiator (`initiator.rs:67-70`), so an agent-run macro step
still writes the note; a human-run macro does not.

### 3c. Note content + placement

`acknowledge_incident` (`incident.rs:1220-1282`): AFTER the successful
`incident_repo.acknowledge(...)` + `cancel_escalation_run(...)` (i.e. only when
state actually changed — the idempotent already-acked early return at `:1246-1248`
writes **no** note), when `cmd.initiator.is_some_and(|i| i.is_agentic())`:

```
"Incident acknowledged by {user_id} via AI agent (initiator: {initiator}) — escalation chain cancelled, paging stopped"
```

`escalate_incident_now` (`incident.rs:1284-1310`): in the `Some(run)` branch
only (the no-active-run early return writes **no** note — nothing happened),
after `fire_escalation_step` returns `Ok`, same gate:

```
"Immediate escalation requested by {user_id} via AI agent (initiator: {initiator})"
```

Where `{user_id} = cmd.actor.user_id` (raw id — matches the C1 fired-step note
precedent, "Escalation step N paged <uuid> …"; the seeded system/bot-user +
username-resolution improvements stay in the tracked backlog) and
`{initiator} = initiator.as_audit_str()` (so a future Plugin/Mcp initiator is
labeled honestly, not falsely as "agent" — the fixed prose says "via AI agent",
the parenthetical says exactly which agentic channel). Phase = current phase or
`Detection` fallback, identical to `:1355-1361`. Failure to write the note:
`tracing::warn!`, command still succeeds (`:1371-1391` precedent). This wording
is a human-readable timeline string, not a parsed contract — but keep it stable;
vision will render it and D2 runbook transcripts will extend the same pattern.

Attribution honesty: the agent runs as the user's `Actor`, so `acknowledged_by`
/ audit rows carry the real user; the note adds the "via AI agent" annotation on
top. Human invocations (GraphQL `initiator: None`, palette/keybind non-agentic
initiators) produce no note — the timeline only records agency, and it does so
exactly once per action.

## §4 `incident.ack` confirmation copy — carrier ruling

Verified: `ConfirmationChallengeDto` (`confirmation.rs:94-98`) and its GraphQL
mirror `ConfirmationChallenge` (`types.rs:666-670`) carry only
`token`/`args_hash`/`expires_at`; the `CommandResult.message` accompanying a
challenge is a generic constant (`dispatch.rs:489`). No human-facing warning
string rides the challenge today.

**Ruling:** the FE sources the warning from **`Command.description`** — already
exposed on the GraphQL `Command` type (`types.rs:487`) and already in the
agent's tool list / palette data. §1d rewrites it to state the consequence
explicitly: *"Acknowledge an incident. This cancels the escalation chain and
stops paging on-call for this incident."* Vision's agent-confirmation modal
should render the command's `description` (falling back to `title`) as the
consequence line. Rationale: the copy is static per-command, so the definition
is its natural home; a per-issuance challenge message field would be an SDL
change (and codegen run) buying nothing until some command needs *dynamic*
challenge copy — if D2 runbooks ever do, add
`ConfirmationChallenge.message: Option<String>` then. `nl_description` (§1d)
carries the same consequence to the model, so the agent is warned before it
even attempts the call. **Action for nick-fury:** relay to vision — modal copy
source = `Command.description`; no new API surface.

## §5 CI inventory update (mandatory, by design)

`collect_ai_callable_inventory_phase4_a5_yields_twelve_commands`
(`rim_infra_services/src/command/registry/mod.rs:136-169`) is a set-equality
security gate pinning exactly the AI-callable IDs. It goes **12 → 16**: add
`"incident.ack"`, `"incident.escalate"`, `"oncall.who"`, `"oncall.take_shift"`
to `expected`. Rename the test (suggested:
`collect_ai_callable_inventory_matches_reviewed_set_c2_sixteen_commands`) and
extend its comment: the 4 C2 additions are per this spec (Jakub-ratified) and
go through the standard hawkeye + black-widow gate, satisfying the "no new
ai_callable builtin without security review" contract the comment states.
CI failing without this edit is the feature working as intended.

## §6 Tests (mandatory — every new/changed function gets tests AND docs)

Patterns to copy: `rim_infra_services/tests/ai_surface_filter_test.rs`,
`rim_infra_services/tests/chat_list_channels_agent_test.rs`,
`rim_infra_services/tests/p5_a4_macro_gates_test.rs`. All async services keep
the boxed-future mock shape (see CLAUDE.md mockable-traits note). No
`unwrap`/`expect` in non-test code; errors stay typed `ServiceError`.

1. **Registry unit tests** (`registry/{incident,oncall}.rs` or registry mod
   tests): each of the four defs has `ai_callable == true`, non-empty
   `nl_description`, 4 intent samples, `required_capabilities.is_empty()`, and
   the **unchanged** sensitivity/dangerous/typed_verb values (pins "no
   downgrade"). §5's set-equality test updated 12→16.
2. **AI surface**: all four appear in `filter_ai_surface` output for an actor
   (they require no caps, so no grants needed — this also pins the §2 ruling);
   an actor still sees them with an empty capability grant environment.
3. **Agent confirmation gate** (integration, `chat_list_channels_agent_test.rs`
   style, agentic initiator):
   - `oncall.who`, `initiator: Agent`, no proof → `Success` (no challenge).
   - Each of `incident.ack` / `incident.escalate` / `oncall.take_shift`,
     `initiator: Agent`, no proof → `ConfirmationRequired` with a populated
     challenge; re-submit with the valid `InBandToken` proof → `Success`, and
     the underlying service method was called exactly once.
   - Regression guard: `incident.ack`, `initiator: Palette` (and `Keybind`),
     no proof → `Success` one-shot (C1 human surfaces unchanged).
4. **Transcript notes** (service unit tests, `incident_unit_tests.rs`):
   - ack with `initiator: Some(Agent)` → `incident_phase_repo.create` called
     exactly once, note contains `"via AI agent"` and
     `"paging stopped"`.
   - ack with `initiator: None` and with `Some(Palette)` → `create` NOT called.
   - already-acknowledged incident + `Some(Agent)` → idempotent return, no note.
   - escalate with `Some(Agent)` + active run → exactly one note; with
     `Some(Agent)` + no active run → no note; with `None` + active run → no
     note (the C1 fired-step note inside `fire_escalation_step` is separate and
     unchanged — assert the agent-attribution note specifically).
   - note-write repo failure → command still returns `Ok` (warn-only path).
5. **Executor threading** (`executor/incident.rs` tests): `ack`/`escalate`
   mock `withf` asserts `cmd.initiator == Some(CommandInitiator::Agent)` when
   invoked with an Agent initiator.
6. **Macro rollup** (`p5_a4_macro_gates_test.rs` pattern, one test): an
   ai_callable macro containing an `incident.ack` step, executed by an Agent
   initiator, requires the per-step confirmation (pins that C2 commands compose
   into agent runbooks under the existing rollup — no new rollup code).

## §7 SDL / GraphQL / schema impact

- **No SDL change. Skip the SDL→codegen procedure entirely.** `ai_callable`,
  `nl_description`, `intent_samples`, `description` are existing fields of the
  GraphQL `Command` type — only their *values* change in `commandDiscover` /
  `listCommands(surface: AI)` output.
- **No DB schema change, no migration.** The timeline note reuses the existing
  `incident_phase` table via `incident_phase_repo.create`. Rollback note: pure
  code revert; no forward-only migration is introduced or required.
- `rim_api/src/mutation.rs` is touched (one line, `initiator: None`) but the
  schema shape is identical.

## §8 Milestones, constraints, docs

**One PR.** Suggested commit order within it: (1) registry flips + §5 test
update, (2) initiator threading + Cmd fields + transcript notes + tests,
(3) docs. Not worth splitting — the pieces are only demoable together.

Constraints (as always): layered architecture (nothing new crosses layers —
the initiator type already lives in `rim_services`), no `unwrap`/`expect` in
library code, typed `ServiceError` (no stringly errors), import ordering,
`cargo fmt` + `clippy -D warnings` clean, hawkeye + black-widow gate before
done (black-widow review is specifically required by the §5 inventory-test
contract).

Docs:
- `docs/oncall.md`: add a **"C2 — AI agent operation"** section: the four
  commands are AI-callable; Safe read is unconfirmed, the three Mutating ones
  always require a human confirmation proof for agentic initiators
  (`requires_confirmation_for_initiator` policy, no exemptions); ack
  confirmation copy explains it stops paging; agent-driven ack/escalate leave
  an incident-timeline transcript note; capabilities intentionally `[]` with
  the §2 rationale and the "revisit when a per-user grant store exists" marker.
- Doc comments: the new Cmd fields (§3b), the new `initiator` params on
  `execute_builtin`/`execute_builtin_command`, and updated header comments in
  `registry/{incident,oncall}.rs` (drop the stale "C2 flips this later" text).

## §9 Explicit non-goals

- No change to `requires_confirmation_for_initiator`, sensitivity tiers,
  macro rollup, sanitizer, or the confirmation store/TTL.
- No server-side agent loop (the agent stays client-side in rim-ui).
- No new capabilities, no `builtin_capabilities()` change.
- No GraphQL `escalateIncidentNow` mutation (command-only today, as in C1).
- No timeline note for `oncall.who` / `oncall.take_shift`.
- No `.expect`-based shortcuts outside the existing registration-time
  developer-error path.
