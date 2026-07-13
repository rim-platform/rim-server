# Track C1 — On-call Schedules + Escalation Policies (implementation spec)

**Author:** iron-man · **Implementer:** hulk · **Requested by:** nick-fury (ratified by Jakub 2026-07-10)
**Strategic frame:** the Track C **spearhead** — the one structural gap Mattermost punts to
PagerDuty/Opsgenie (`docs/research/mattermost-playbooks-teardown-2026-07-10.md`). "incident.io in the
box." Build it native, first-class, self-hosted.

## Scope & shape
Two coupled subsystems plus their wiring into incidents:
- **On-call schedules** — rotations, ordered participants, timezone-correct handoffs, overrides, and a
  **pure** "who is on call now" resolver.
- **Escalation policies** — ordered timed steps targeting a user or a schedule's current on-call,
  performing mention + email actions, cancelled by acknowledgement.
- **Wiring** — incident declare (manual + `ingest_alert`) arms the escalation timer chain; ack/resolve
  cancels it; a single-node timer wheel behind a trait with boot-recovery from DB state.

This is an **XL** feature. Land it in three reviewable milestones (see §12) — do not attempt one PR.

## Constraints (as always)
Layered architecture (rim_core traits → rim_infra_services impls → rim_api GraphQL; timer wheel in the
composition root like the pipeline evaluator). Forward-only migrations (`sea migrate generate`), entity
hand-merge per CLAUDE.md. No `unwrap`/`expect` in library code; typed `ServiceError`. fmt / clippy -D
warnings / full + feature-gated test suites green. Tests **and** docs for everything new. Frozen-contract
discipline on events/SDL: PascalCase serde tags, snake_case fields, `#[serde(skip_serializing)]` for
server-internal fields; FE builds against the §shapes here — coordinate any change through nick-fury.

---

## §0 Existing-schema friction (flagged per the ask)

Things in the current schema that shape or fight this design — read before implementing:

1. **No `ack` concept anywhere.** Clean add; I make ack orthogonal to status (§4), not a new status.
   No fight — just net-new fields.
2. **No system/bot user, and messages require a real member author.** `SendMessageCmd.actor` needs a
   real `user_id` that passes `ensure_conversation_member_access`; `Principal::bot/webhook` carry
   `user_id: None` and cannot author. This *fights* a clean "escalation bot posts the page" design. My
   v1 decision (§5): author the escalation mention message as the incident's **`started_by`** user
   (always a real member — the human declarer, or the ingest token's `created_by`). A seeded system
   user is the right long-term fix; flagged as follow-up, **not** built in C1.
3. **Users have no timezone.** Rotation handoff math needs a timezone; I put it on the **schedule**
   (§1), not the user — a rotation has one handoff clock. Optional `user.timezone` added only for
   display/defaulting.
4. **`chrono-tz` is not a dependency.** IANA tz math for DST-correct handoffs needs it — new dep (§1).
5. **Timeline note = an `incident_phase` row with a `note`** (not a message/tag). Escalation step
   firing both posts the mention message *and* appends an `incident_phase` timeline note, reusing that
   mechanism (§5).
6. **`idx_incident_active_conversation`** (one active incident per conversation) means one incident ⇒
   one escalation run — no fan-out ambiguity. No fight.
7. **Single-node in-memory infra is the documented stance** (`fable-notes.md` §7: broadcaster,
   ConfirmationStore, rate limiters — "swap for Redis later, behind traits"). The timer wheel follows
   that exactly (§6): in-memory now, trait boundary, DB-backed run state for crash recovery, documented
   multi-node caveat (a lost in-memory timer = a missed page).

---

## §1 On-call schedules — model & timezone

New dependency: `chrono-tz = "0.10"` (IANA tz database) in `rim_core` and `rim_infra_services`. Validate
IANA names via `chrono_tz::Tz::from_str`.

Core models (`rim_core/src/oncall/model.rs`, `model_id!` for ids):
```rust
pub struct OnCallSchedule {
    pub id: OnCallScheduleId, pub server_id: ServerId, pub name: String,
    pub timezone: String,               // IANA, e.g. "America/Chicago" — validated on write
    pub created_by: UserId, pub created_at: DateTime<Utc>, pub updated_at: DateTime<Utc>,
}
pub struct RotationLayer {
    pub id: RotationLayerId, pub schedule_id: OnCallScheduleId, pub name: String,
    pub priority: i32,                  // higher priority layer wins where it has coverage
    pub kind: RotationKind,
    pub anchor_at: DateTime<Utc>,       // when participant[0]'s first shift starts (UTC instant)
    pub created_at: DateTime<Utc>,
}
pub enum RotationKind {
    Daily  { handoff_time: NaiveTime },              // hand off every day at local wall-clock time
    Weekly { handoff_weekday: Weekday, handoff_time: NaiveTime }, // every 7 days at local time
    Custom { interval_seconds: i64 },                // fixed UTC duration (DST-agnostic, documented)
}
pub struct RotationParticipant { pub id: ..., pub layer_id: RotationLayerId, pub user_id: UserId, pub position: i32 }
pub struct OnCallOverride {
    pub id: OnCallOverrideId, pub schedule_id: OnCallScheduleId, pub user_id: UserId,
    pub start_at: DateTime<Utc>, pub end_at: DateTime<Utc>, pub created_by: UserId,
}
```

**Timezone design decision (document it):** rotation-boundary math lives in the **schedule's** timezone.
`Daily`/`Weekly` hand off at a *wall-clock* local time so the handoff doesn't drift an hour across DST;
`Custom` is a pure UTC duration (no wall-clock alignment; simplest, and correct for "every 6 hours"
semantics) — say so in docs. Per-user timezone for follow-the-sun (restricting a participant to their
working hours) is explicitly a **future layer**, not v1: mixing per-participant tz into boundary math
makes "who's on now" ambiguous at handoff. Add optional `user.timezone` (§8 migration) used only for
(a) rendering on-call times in the viewer's local tz on the FE and (b) defaulting a new schedule's tz.

## §2 The pure resolver (property-testable core)

The heart of C1. A pure function, no I/O, in `rim_core/src/oncall/resolve.rs`:
```rust
/// Everything needed to answer "who is on call at `now`" — passed by value/ref, no repos.
pub struct ScheduleSnapshot<'a> {
    pub timezone: Tz,
    pub layers: &'a [ResolvedLayer<'a>],   // each: priority, kind, anchor_at, ordered participants
    pub overrides: &'a [OnCallOverride],
}
/// Resolve the effective on-call user at `now`. Overrides beat layers; among layers, the highest
/// `priority` with a participant covering `now` wins. `None` if no layer has participants.
pub fn resolve_on_call(snapshot: &ScheduleSnapshot, now: DateTime<Utc>) -> Option<UserId>;

/// The shift index for a single layer at `now`: the count of handoff boundaries between `anchor_at`
/// and `now`, computed in the schedule tz for Daily/Weekly (so DST adds/removes no phantom shift),
/// as `floor((now-anchor)/interval)` for Custom. `participant = participants[index mod len]`.
pub fn layer_on_call(layer: &ResolvedLayer, tz: Tz, now: DateTime<Utc>) -> Option<UserId>;
```
Resolution order: (1) an `OnCallOverride` with `start_at <= now < end_at` wins outright (latest-created
wins on overlap — document the tie-break); (2) else the highest-`priority` layer whose participant list
is non-empty yields `layer_on_call`. Empty schedule → `None`.

**Range enumeration (FLAG B — server-resolved shift strip; keeps "FE never computes rotation math"):**
add a second pure function so the client can render a shift timeline without any rotation math:
```rust
pub struct OnCallShift { pub user_id: UserId, pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>, pub is_override: bool }
/// All contiguous shifts intersecting [from, to), in chronological order. Walks handoff boundaries
/// (§ algorithm below) between `from` and `to`, applying override precedence by splicing override
/// windows over the rotation shifts. Bound the span server-side (reject `to - from` > MAX_RANGE, e.g.
/// 90 days) so a caller can't force an unbounded boundary walk.
pub fn resolve_shifts(snapshot: &ScheduleSnapshot, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<OnCallShift>;
```
`resolve_shifts` is `resolve_on_call` integrated over a range — same boundary math, so it shares the
DST handling and is covered by the same property invariants (contiguous, non-overlapping, covers the
range, overrides splice in). Property-test it alongside the point resolver (§7).

**Handoff-boundary algorithm (Daily/Weekly), spell it out for hulk:** convert `anchor_at` and `now` to
the schedule tz; count how many local handoff instants (Daily: each day at `handoff_time`; Weekly: each
`handoff_weekday` at `handoff_time`) lie in `(anchor_local, now_local]`. That count = elapsed shifts.
Use chrono-tz local date iteration; handle the DST "spring-forward gap" (a 02:30 handoff on a day that
jumps 02:00→03:00) by resolving the gap forward to the next valid instant, and the "fall-back overlap"
by taking the first occurrence — document both choices.

**Mandated property tests** (§7) target *invariants* of this function, not a reference impl.

## §3 Escalation policies — model

`rim_core/src/escalation/model.rs`:
```rust
pub struct EscalationPolicy { pub id: EscalationPolicyId, pub server_id: ServerId, pub name: String,
    pub created_by: UserId, pub created_at: ..., pub updated_at: ... }
pub struct EscalationStep {
    pub id: EscalationStepId, pub policy_id: EscalationPolicyId, pub position: i32,
    pub delay_seconds: i64,             // wait BEFORE this step, measured from the previous step firing
                                        // (or from declare for position 0; 0 = fire immediately)
    pub target: EscalationTarget,
    pub actions: Vec<EscalationAction>, // AND-combined; at least one required
}
pub enum EscalationTarget {
    User(UserId),
    OnCall(OnCallScheduleId),           // resolve via §2 at fire time
    // future: Webhook(...)  — reserve the enum slot; do not implement in C1
}
pub enum EscalationAction { MentionInChannel, Email /* future: Push (Track D4) */ }
```
`EscalationAction` is deliberately an **open enum with a reserved `Push`** so Track D4 slots in without a
migration to the action shape (store as a small string set / bitmask — see §8). Do not add `Push`
handling now; just don't design it out.

Channel binding: `conversation_escalation_policy` (conversation_id PK → policy_id) is the **default**
policy for incidents declared in that channel. `StartIncidentCmd` gains an optional
`escalation_policy_id` override; `ingest_alert` uses the channel default (no human to choose). The
resolved policy id is stored on the incident (§8).

## §4 Acknowledgement semantics

**Decision: ack is orthogonal to status, expressed as two new incident fields — not a status variant.**
Add `acknowledged_at: Option<DateTime<Utc>>` and `acknowledged_by: Option<UserId>` to `Incident`.
Rationale: `IncidentStatus` (Active/Mitigated/Resolved) is the *lifecycle*; ack means "a human has this,
stop paging" and can coexist with Active. Folding ack into status would either add an `Acknowledged`
status that muddies the mitigate/resolve transitions or force premature mitigation. incident.io/PagerDuty
keep ack separate — mirror that.

New service method `acknowledge_incident(cmd) -> ServiceResult<IncidentDto>`:
- Requires `ensure_server_member(actor)` (any member can ack; not owner-gated — whoever responds).
- Idempotent: if already acked, return the incident unchanged (don't error, don't re-page).
- Only meaningful while `status == Active`; acking a Resolved incident is a no-op-ish Validation-free
  return (document).
- Sets `acknowledged_at/by`, **cancels the escalation run** (§6), emits `IncidentAcknowledged` (§9).

`update_incident_status` → Mitigated/Resolved and `resolve_incident` also **cancel the escalation run**
(reaching mitigation/resolution implies someone's handling it). Ack does not change status.

## §5 Step execution (the actions)

When a step fires (from the timer, §6), in a background context with no request actor:
1. Reload the `escalation_run` + incident; **if run status != Active, no-op** (handles ack/resolve races).
2. Resolve the step target → concrete `Vec<UserId>`: `User(u)` → `[u]`; `OnCall(sched)` → load the
   schedule snapshot and call `resolve_on_call(now)` → 0 or 1 user (log + skip actions if `None`, i.e.
   an empty schedule — still advance the chain).
3. Perform each action best-effort (a failure logs `warn` and does not abort the chain):
   - **MentionInChannel:** post a message into `incident.conversation_id` whose `text` contains
     `<@{uuid}>` for each target (mentions auto-parse + fan out — see the send path). **Author =
     `incident.started_by`** (a guaranteed member; §0.2). Construct `Actor::new(incident.started_by)`.
     If that user has since left the server the post fails member-check → log warn, continue (email
     still goes). Add `message_service` + (implicitly, via send_message) mention handling to
     `IncidentRepos`/the service — the incident service does **not** currently hold a `MessageService`,
     so thread `Arc<dyn MessageService>` into it.
   - **Email:** resolve each target's `User.email` (`user_repo.get_by_ids`) and send via the B3
     `EmailSender` using a new `templates::escalation_step(...)` (severity, title, incident link, step
     number). Reuse the `notify_owners_of_declared_incident` pattern (incident.rs:89) for lookup + link.
4. Append an `incident_phase` timeline note (`incident_phase_repo.create(incident_id, current_phase,
   Some("Escalation step N paged <user> via mention+email"))`) so the incident timeline records the page.
5. Advance the run: if a next step exists, set `current_step_index += 1`, `next_fire_at = now +
   next.delay_seconds`, persist, re-arm the scheduler; else set run `status = Completed`.

All action failures are non-fatal (paging is best-effort; never panic a timer task). Escalation email
bodies must use the **HTML-escaping** template helper (the B3 security batch added escaping — reuse it;
do not hand-concatenate unescaped incident titles into HTML).

## §6 Timer wheel behind a trait + boot recovery

**Run state is DB-backed (source of truth); the in-memory wheel is only the timing mechanism.**

`escalation_run` table (§8): `id, incident_id, policy_id, current_step_index, next_fire_at, status
(active|cancelled|completed), created_at, updated_at`. One active run per incident (partial unique on
`incident_id WHERE status='active'`).

Trait (`rim_services/src/escalation/scheduler.rs`):
```rust
#[async_trait] pub trait EscalationScheduler: Send + Sync {
    /// Arm (or re-arm) a run to fire at `fire_at`. Idempotent per run_id.
    fn arm(&self, run_id: EscalationRunId, fire_at: DateTime<Utc>);
    /// Cancel a pending timer (ack/resolve). No-op if not armed.
    fn cancel(&self, run_id: EscalationRunId);
}
```
In-memory impl (`rim_infra_services/src/escalation/in_memory_scheduler.rs`): a `DashMap<EscalationRunId,
fire_at>` plus a driver task. The **driver task** lives in `app.rs` following the pipeline-evaluator
pattern exactly (clone handles before the closure; `shutdown_tx.subscribe()`; `tokio::select!` a
`tokio::time::sleep_until(next_deadline)` against `shutdown_rx.changed()` and an "arm changed" notify).
On each fire it calls back into a `EscalationExecutor` (the incident service, behind a trait) to run the
step (§5). Keep the wheel dumb — it only knows (run_id, when); the executor owns the logic.

**Boot recovery (the recommended design):** on startup, before/around the driver spawn, load all
`escalation_run` rows with `status='active'` and `arm(run_id, next_fire_at)` for each. A `next_fire_at`
already in the past fires ASAP (a missed page during downtime pages immediately on recovery — correct).
This makes a node restart lose nothing: the DB row is the truth, the wheel is rebuilt from it.

**Idempotency/races:** every fire reloads the run and bails if `status != active` (ack/resolve won the
race). Advancing a step persists `current_step_index` + `next_fire_at` in one update, so a crash between
firing and persisting at worst re-pages one step on recovery (at-least-once paging — the safe direction
for an alerting system; document it).

**Multi-node caveat (document, mirror `fable-notes.md` §7):** two nodes would each arm and double-page.
Single-node only for now; the trait is the swap point for a DB-`SELECT ... FOR UPDATE SKIP LOCKED`
poller or a Redis/`sidekiq`-style scheduler later. Note this is an *availability/UX* regression under
naive multi-node, not a security one.

## §7 Mandated tests
- **Pure resolver property tests** (`rim_core`, using `proptest` — add as a dev-dependency if absent):
  - *Monotonic within a shift:* for random schedules, `resolve_on_call` is constant between consecutive
    handoff boundaries and changes only at them.
  - *Round-robin coverage:* over N·interval, each of N participants is on call for exactly one shift per
    cycle, in `position` order.
  - *DST correctness:* a Weekly schedule across a spring-forward and a fall-back weekend hands off
    exactly once that week (no skipped/duplicated shift); assert the handoff wall-clock time is stable.
  - *Override precedence:* an override covering `now` always wins over any layer; layered priority
    resolves highest-first; empty schedule → `None`.
  - Include explicit unit cases for the two DST edge instants (gap/overlap) with the documented choices.
  - *`resolve_shifts` consistency (FLAG B):* over a random range, the returned shifts are chronological,
    contiguous (no gap), non-overlapping, cover exactly `[from, to)`, and for any instant `t` in range the
    shift containing `t` names the same user as `resolve_on_call(t)` — this ties the range resolver to the
    point resolver so they can never diverge. Overrides appear as `is_override` shifts spliced over the
    rotation.
- **Escalation chain tests** (service, mocked scheduler + repos): declare → step0 fires → step1 armed at
  correct `next_fire_at`; **ack cancels** remaining steps (no further fires); resolve/mitigate cancels;
  a fire after cancel is a no-op (status-reload guard); `OnCall` target with empty schedule skips actions
  but advances.
- **Boot-recovery test** (db-tests, loud `db_conn()`): persist an active run with a past `next_fire_at`,
  run the recovery routine, assert it arms + fires. Persist a `cancelled` run, assert recovery ignores it.
- **who-is-on-call db-test:** persist a schedule + rotation + override, assert the service `who_is_on_call`
  matches the pure resolver over the same inputs (integration wiring correctness).
- **Ack semantics tests:** idempotent ack; ack of Active sets fields + cancels; ack doesn't change status;
  resolve after ack works.
- Add all new tables to the db-tests `reset_db`/TRUNCATE set.

## §8 Schema (migrations — forward-only, `sea migrate generate`)

One migration per cohesive group (or a few); register each in `migration/src/lib.rs`; hand-merge each
entity + `*_mapper.rs`. Rollback = drop in reverse FK order.

1. `create_oncall_schedules`: `on_call_schedule`, `on_call_rotation_layer`
   (FK schedule cascade; `kind` as `rotation_kind varchar` + `interval_seconds bigint null`,
   `handoff_time time null`, `handoff_weekday smallint null`, `anchor_at timestamptz`, `priority int`),
   `on_call_rotation_participant` (FK layer cascade, FK user; unique `(layer_id, position)`),
   `on_call_override` (FK schedule cascade, FK user; index `(schedule_id, start_at, end_at)`).
2. `create_escalation_policies`: `escalation_policy`, `escalation_step` (FK policy cascade; `position`,
   `delay_seconds bigint`, `target_kind varchar`, `target_user_id uuid null`, `target_schedule_id uuid
   null`, `actions varchar` storing a sorted CSV/bitmask of action kinds; unique `(policy_id, position)`;
   CHECK that exactly one of target_user_id/target_schedule_id is set per `target_kind`),
   `conversation_escalation_policy` (`conversation_id` PK, `server_id`, `policy_id` FK).
3. `create_escalation_run`: `escalation_run` (FK incident cascade; `policy_id`, `current_step_index int`,
   `next_fire_at timestamptz null`, `status varchar` default `'active'`; **partial unique**
   `(incident_id) WHERE status='active'`; index `(status, next_fire_at) WHERE status='active'` for the
   boot-recovery scan).
4. `add_incident_escalation_and_ack_fields`: `ALTER TABLE incident ADD COLUMN escalation_policy_id uuid
   NULL`, `acknowledged_at timestamptz NULL`, `acknowledged_by uuid NULL`. Nullable, no backfill (existing
   incidents were never escalated/acked — correct as NULL).
5. `add_user_timezone`: `ALTER TABLE "user" ADD COLUMN timezone varchar(64) NULL` (display/defaulting only).

Rollback notes on each: all additive; `down` drops tables/columns in reverse. The incident and user
alters are nullable `ADD COLUMN` (non-rewriting on PG11+). No data migration.

## §9 Events (frozen contract — coordinate with vision via nick-fury)
Add to `WorkspaceEvent` (`rim_services/src/events/types.rs`), matching the B2 incident-variant shape
(carry `conversation_type` for the channel-visibility bypass; `#[serde(skip_serializing)]
member_user_ids`):
```rust
EscalationStepFired {
    workspace_id: ServerId, conversation_id: ConversationId, conversation_type: String,
    incident_id: IncidentId, escalation_policy_id: EscalationPolicyId, step_index: i32,
    target_user_ids: Vec<UserId>,           // serialized — FE shows who was paged
    #[serde(skip_serializing)] member_user_ids: Vec<UserId>,
}
IncidentAcknowledged {
    workspace_id: ServerId, conversation_id: ConversationId, conversation_type: String,
    incident_id: IncidentId, acknowledged_by: UserId,
    #[serde(skip_serializing)] member_user_ids: Vec<UserId>,
}
```
Extend the `workspace_id()` and `member_user_ids()` exhaustive matches (compiler-enforced) and the
subscription filter (`rim_api/src/subscription.rs`) with the channel-bypass pattern. Emit
`EscalationStepFired` after each step executes, `IncidentAcknowledged` from `acknowledge_incident`.

## §10 GraphQL contract + SDL delta (propose to vision via nick-fury)

**Authorization (ruled by nick-fury):** schedule/policy/override **editing** (all create/update/delete
mutations) is gated on **Owner + Moderator** — use `ensure_server_owner_or_moderator` (add this helper to
`rim_infra_services/src/util.rs` alongside `ensure_server_owner`, checking `member_role ∈ {Owner,
Moderator}`). Per-channel moderator roles do **not** apply: schedules/policies are server-scoped, so the
server role governs. Guests are read-only. `acknowledgeIncident` and all read queries: any server member.
The FE gates edit affordances on the new `Server.viewerRole` field (see `docs/specs/server-viewer-role.md`,
which can land ahead of this) — display-only; the service-layer role check here is the real boundary.

Additive (no breaking changes). Mutations/queries:
- Schedules: `createOnCallSchedule`, `updateOnCallSchedule`, `deleteOnCallSchedule`, `addRotationLayer`,
  `updateRotationLayer`, `removeRotationLayer`, `setRotationParticipants(layerId, [userId] ordered)`,
  `createOnCallOverride`, `deleteOnCallOverride`; queries `onCallSchedules(serverId)`,
  `whoIsOnCall(scheduleId): OnCallNow` (current user + shift end + next user).
- **FLAG A — `serverOnCall(serverId): [OnCallNow!]`**: current on-call across ALL schedules in the server
  in one call, so the always-visible status bar doesn't N+1 over `whoIsOnCall`. Loads the server's
  schedules and resolves each via §2. Keep per-schedule `whoIsOnCall` too.
- **FLAG B — `onCallShifts(scheduleId: ID!, from: DateTime!, to: DateTime!): [OnCallShift!]!`**: the
  server-resolved shift strip, backed by `resolve_shifts` (§2). Reject `to-from > 90d` with a Validation
  error. `OnCallShift { user: User!, startAt, endAt, isOverride }` — nested `user` (FLAG F pattern).
- Policies: `createEscalationPolicy(name, steps:[EscalationStepInput])`, `updateEscalationPolicy`,
  `deleteEscalationPolicy`, `setChannelEscalationPolicy(conversationId, policyId)`; query
  `escalationPolicies(serverId)`.
- Incident: `acknowledgeIncident(incidentId): Incident`; the `Incident` type gains
  `escalationPolicyId: ID`, `acknowledgedAt: DateTime`, `acknowledgedBy: ID`, `acknowledgedByUser: User`.
- **FLAG C — `Incident.escalationRun: IncidentEscalationRun` (nullable, read-only)**: a read projection of
  the DB `escalation_run` row so a client loading mid-incident or after reconnect can render the live
  "escalating to L2 in 4:32" countdown (the `EscalationStepFired` event alone can't give `nextFireAt` to a
  fresh client). `IncidentEscalationRun { currentStepIndex: Int!, nextFireAt: DateTime, status:
  EscalationRunStatus! }`. Nullable = no policy attached / run completed. Pure read projection — run state
  is already DB-backed in this design, so this adds a resolver, not persistence.
- **FLAG F — `OnCallNow` exposes nested `currentUser: User`, `nextUser: User` (+ `shiftEndsAt:
  DateTime`)**, not just ids, resolved via the existing `UserLoader` DataLoader (same pattern as
  `message.author`) so avatars/names render without a second round-trip. Apply the same nested-`User`
  treatment to `OnCallShift.user` (FLAG B) and `acknowledgedByUser`. Use `UserLoader` for all of these to
  avoid N+1 across `serverOnCall`/`onCallShifts`.
- Types: `OnCallSchedule`, `RotationLayer` (with `kind` union/enum + fields), `RotationParticipant`,
  `OnCallOverride`, `OnCallNow`, `OnCallShift`, `IncidentEscalationRun`, `EscalationRunStatus` enum
  (`ACTIVE`/`CANCELLED`/`COMPLETED`), `EscalationPolicy`, `EscalationStep`, `EscalationTarget` (enum kind +
  optional user/schedule), `EscalationActionKind` enum (`MENTION_IN_CHANNEL`, `EMAIL`; `PUSH` reserved).

### §10.1 FLAG E — lifecycle command registration (C1 registers, C2 fuses) — my architecture ruling

**Ruling: register the four commands in C1 (nick-fury's primary proposal), not the hardcoded-keybind
fallback. It fits the command/keybind model cleanly, and the fallback would be the worse choice.**

Register in the builtin command registry (new `command/registry/{incident,oncall}.rs` + thin
`command/executor/{incident,oncall}.rs`, wired into the dispatch match — the established per-domain
executor pattern):
| command id | sensitivity | keybind_capable | ai_callable (C1) | calls |
|---|---|---|---|---|
| `oncall.who` | Safe | yes | **false** | oncall service `who_is_on_call` |
| `oncall.take_shift` | Mutating | yes | **false** | create a short override for the actor |
| `incident.ack` | Mutating (`dangerous:false`) | yes | **false** | `acknowledge_incident` |
| `incident.escalate` | Mutating | yes | **false** | trigger the next escalation step now |

**`incident.ack` is single-action on every human surface — verified against the gate, not assumed.**
`CommandDefinitionDto::requires_confirmation_for_initiator` (`rim_services/src/command/dto.rs:228`)
prompts (returns a `ConfirmationRequired` challenge) only when: `dangerous == true` (any initiator),
OR `sensitivity == Restricted`, OR `initiator.is_agentic() && sensitivity >= Mutating`. A **non-agentic**
initiator (Palette, Keybind, ContextMenu, Cli — the banner button, the paged-toast inline ack, and the
configurable keybind all fall here) with a plain `Mutating`, `dangerous:false` command does **not** get a
proof challenge → one-shot ack. The `acknowledgeIncident` GraphQL mutation has no confirmation gate at all.
So keep `incident.ack = Mutating, dangerous:false` (the honest sensitivity for a state transition — do NOT
downgrade to Safe just to dodge a prompt that isn't there; Safe would mislabel a mutation for audit).
**C2 note:** the *only* surface where this would prompt is the future agent path (agentic + Mutating). That
is a deliberate async agent confirmation-proof flow, not a human modal, and whether to exempt the agent
from ack-confirmation is a C2 decision — flag it there; it does not affect any C1 surface.

Why this is the clean split, not a compromise:
- **It's the same dispatch, two entry points.** Each executor just calls the *same* service method the
  GraphQL mutation calls (e.g. `incident.ack` → `acknowledge_incident`) — no duplicated logic, exactly the
  unified-dispatcher philosophy the Reframe established (agent tool-calls already route through this
  dispatcher). Vision's ack keybind binds to `incident.ack` like any other keybind (the keybind model
  validates `keybind_capable` — a "hardcoded ack keybind" outside the registry would be a special-case
  bypass of that model, precisely the invariant erosion we avoid).
- **Composability is free.** Being a registered command already makes these macro/pipeline-referenceable
  in C1 — that falls out of registration, no extra work.
- **C2 is a field/behavior upgrade, not a re-architecture.** C2 flips `ai_callable: true`, adds
  `nl_description`/`intent_samples` for intent routing, and layers the agent sensitivity-rollup +
  confirmation — all *on top of* these existing definitions. Nothing built in C1 is thrown away. The
  base `sensitivity` set here (Safe/Mutating) is already correct for human invocation (Mutating commands
  hit the existing confirmation gate on keybind/palette invoke); C2 doesn't change it.
- **The fallback (hardcoded keybind + deferred registration) is strictly worse:** it builds a throwaway
  keybind path C2 rips out, and special-cases the keybind model. Only take it if command registration
  somehow blocks — it doesn't.

**No init cycle (important — don't reach for OnceLock):** unlike the command↔macro case, none of the
incident/oncall/escalation services depend on `command_service`, so build them **before**
`command_service` in `provider.rs` and inject their `Arc`s straight into `CommandServices`. hulk should
move incident/oncall/escalation construction above the command-service build (safe — no back-dependency),
not replicate the macro OnceLock late-binding. Land this in **C1b** (when `acknowledge_incident` + the
escalation trigger exist); `oncall.who`/`oncall.take_shift` need only the C1a schedule service but keep
all four together as one registration unit in C1b so the executors land once. Keybind targets are then
available before C1c's UI needs them.
Enum-casing note (CLAUDE.md): Rust `Sev1`→`SEV_1`, so name action variants to serialize as the intended
SCREAMING_SNAKE. Hand nick-fury the full delta before implementing the resolvers so vision designs the
on-call/escalation UI against a frozen shape. Regenerate `schema.graphql` at the end.

## §11 Config
Add (config.rs, `Option<T>` + accessor + default): `escalation_timer_resolution_secs` (default 5 — how
often the driver wakes to check for due timers if you poll rather than `sleep_until`; if you use
`sleep_until` to the next deadline, this is only a floor — document which). `RIM_`-prefixed. No secrets.

## §12 Suggested milestones (land as 3 reviewable PRs)
- **C1a — schedules + pure resolvers:** models, migrations 1 + 5, chrono-tz, `resolve_on_call` **and
  `resolve_shifts`** + the full property-test suite, schedule/rotation/override service CRUD + db-tests,
  and the read queries that need only schedules — `whoIsOnCall`, `serverOnCall` (FLAG A), `onCallShifts`
  (FLAG B) — with nested `User` via `UserLoader` (FLAG F). Owner+Moderator edit gate
  (`ensure_server_owner_or_moderator`). No timers, no escalation. De-risks the tz math first;
  independently shippable/demoable.
- **C1b — escalation policies + ack + timer wheel + commands:** models, migrations 2/3/4, policy CRUD, ack
  semantics, the `EscalationScheduler` trait + in-memory impl + driver task + boot recovery, step
  execution (mention via `started_by` author + email), wiring into `start_incident`/`ingest_alert`/
  ack/resolve, events, chain + recovery tests, **and the four lifecycle command registrations (§10.1)**
  — build incident/oncall/escalation services before `command_service` and inject (no OnceLock).
- **C1c — remaining GraphQL surface + FE coordination:** `acknowledgeIncident`, Incident ack/policy
  fields + `escalationRun` projection (FLAG C), escalation policy CRUD resolvers, event subscription-filter
  arms, full SDL delta to vision, `docs/oncall.md`. (`Server.viewerRole` lands separately, ahead of this —
  `docs/specs/server-viewer-role.md`.)

## §13 Docs
`docs/oncall.md`: schedule/rotation model, the timezone/DST handling decision, override precedence, the
escalation step/target/action model, ack semantics (orthogonal to status), the single-node timer + boot
recovery + multi-node caveat, and the `started_by`-as-author limitation with the system-user follow-up.
Doc comments on every new trait/struct/method, especially the pure resolver (state the invariants the
property tests enforce) and the scheduler trait (state the at-least-once paging guarantee).

## §14 Follow-ups (explicitly out of C1 scope)
**Seeded system/bot user** (TRACKED follow-up, nick-fury-approved) — the eventual fix for escalation-post
attribution: today escalation mentions are authored as `incident.started_by` (§0.2, §5); a real seeded
system user gives cleaner attribution AND unblocks proper bot/webhook message authoring
(`Principal::bot/webhook` currently can't author because they carry `user_id: None`). Worth a dedicated
spec after C1. Other follow-ups: per-participant follow-the-sun timezones;
`Webhook` escalation target; `Push` action (Track D4); repeat/re-escalation loops and escalation to a
"next" on-call; schedule ICS import/export. Note each in `docs/oncall.md`.
