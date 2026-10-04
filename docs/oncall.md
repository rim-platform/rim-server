# On-Call Schedules

RIM's on-call subsystem answers "who is on call right now" for a team, with timezone-correct
rotation handoffs and one-off overrides. Track C1 — the Track C spearhead
(`docs/research/mattermost-playbooks-teardown-2026-07-10.md`): "incident.io in the box."

**Status:** this document covers **Track C1a** (schedules + the pure resolver + CRUD +
`whoIsOnCall`) and **Track C1b** (escalation policies, acknowledgement, the timer wheel, and the
four lifecycle builtin commands), both landed 2026-07-10. The full GraphQL surface for escalation
(policy CRUD resolvers, `acknowledgeIncident`, the `Incident.escalationRun` projection) and
frontend are **Track C1c** — see "Escalation wiring" below for exactly what is and isn't exposed
via GraphQL yet.

## Workspace feature gate (Track 0.4)

On-call schedules, escalation policies and incidents together form the optional per-workspace
**`incidents`** module (`docs/specs/track-0-4-optional-incident-module.md`). New workspaces have
it off by default; workspaces that existed before the `add_workspace_feature` migration were
switched on by its data step. Only the server Owner can toggle it (`setWorkspaceFeature`,
audited). With the feature off:

- **Writes are rejected** with `FEATURE_DISABLED`: every on-call schedule / rotation layer /
  participant / override mutation, every escalation policy mutation (including
  `setChannelEscalationPolicy`), and every incident mutation (start, status, phase, tag,
  postmortem, resolve, acknowledge, escalate now, alert ingest). The gate runs after the
  authorization check, so a non-member still gets `NOT_FOUND` and never learns the flag.
- **Reads of existing data stay allowed** (schedules, layers, shifts, `whoIsOnCall`, policies,
  incident history and timelines), so history still renders after the module is turned off.
  The exception is the active-incident lookup for a conversation, which returns `null` so the
  incident banner disappears.
- The four builtins `incident.ack`, `incident.escalate`, `oncall.who` and `oncall.take_shift`
  are hidden from the palette, keybind picker, context menu and AI surface, and rejected at
  execution (see `docs/command_model.md`, "`feature`").
- A due escalation step pages nobody: `fire_escalation_step` logs and skips it, leaving the run
  `Active` and untouched. It is not re-armed until the next boot-time recovery sweep.

No data is deleted when the feature is turned off; turning it back on restores everything.

## Model

- **`OnCallSchedule`** — a named, timezone-scoped on-call rotation for a server. `timezone` is an
  IANA name (e.g. `"America/Chicago"`), validated with `chrono_tz::Tz::from_str` on every write.
- **`RotationLayer`** — one rotation within a schedule. Layers have a `priority`; the
  highest-priority layer with at least one participant wins coverage at any given instant (lower
  layers act as a fallback only when a higher layer is completely empty, not when a higher layer
  simply doesn't have a scheduled shift right now — every non-empty layer is always "on" someone).
- **`RotationParticipant`** — an ordered (`position`) member of a layer's rotation. Replaced as a
  whole list via `setRotationParticipants` (delete-then-insert, transactional).
- **`OnCallOverride`** — a one-off `[start_at, end_at)` window where a specific user overrides
  whatever the layers would otherwise say, for the whole schedule (not per-layer).

## Timezone & DST design

Rotation handoff math runs in the **schedule's** timezone, not any per-user timezone — a rotation
has exactly one handoff clock shared by every participant. Per-participant "follow-the-sun"
timezones are an explicit **non-goal for C1** (see "Follow-ups" below): mixing per-participant tz
into the boundary math makes "who's on now" ambiguous right at a handoff.

Each `RotationKind` picks a different notion of "boundary":

- **`Daily { handoff_time }`** — hands off every calendar day at `handoff_time` **local
  wall-clock** time in the schedule's timezone.
- **`Weekly { handoff_weekday, handoff_time }`** — hands off every 7 days, on `handoff_weekday` at
  `handoff_time` local wall-clock time.
- **`Custom { interval_seconds }`** — hands off every `interval_seconds` of elapsed **UTC**
  duration. No wall-clock alignment at all — this is the right semantics for "every 6 hours"
  style rotations, and it's DST-agnostic by construction.

`Daily`/`Weekly` use wall-clock time specifically so a handoff doesn't silently drift by an hour
across a DST transition (an on-call handoff meant for "9am, whoever's turn it is" should still
happen at 9am local time the week clocks change).

### The two DST edge cases

Converting a `NaiveTime` + calendar date to a concrete UTC instant in a timezone can hit two edge
cases once or twice a year:

- **Spring-forward gap** — the wall-clock instant doesn't exist (e.g. `02:30` on a day that jumps
  `02:00 → 03:00`). **Chosen behavior: resolve forward to the next valid instant** (so a `02:30`
  handoff effectively fires at `03:00` that day).
- **Fall-back overlap** — the wall-clock instant occurs twice (e.g. `01:30` on a day that repeats
  an hour). **Chosen behavior: take the first (earlier UTC) occurrence.**

Both are implemented in `rim_core::oncall::resolve::resolve_local` (private to that module) and
covered by explicit unit tests for both edge instants, plus two full-week `Weekly` tests proving a
schedule hands off **exactly once** across a spring-forward and a fall-back weekend (no skipped or
doubled shift).

## The pure resolver

`rim_core::oncall::resolve` is a **pure, I/O-free** module — the "de-risk the hardest part first"
piece of C1a. Its two public entry points:

```rust
pub fn resolve_on_call(snapshot: &ScheduleSnapshot, now: DateTime<Utc>) -> Option<UserId>;
pub fn layer_on_call(layer: &ResolvedLayer, tz: Tz, now: DateTime<Utc>) -> Option<UserId>;
pub fn next_boundary(layer: &ResolvedLayer, tz: Tz, now: DateTime<Utc>) -> Option<DateTime<Utc>>;
pub fn resolve_shifts(snapshot: &ScheduleSnapshot, from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<OnCallShift>;
```

**Resolution order:**
1. Any `OnCallOverride` with `start_at <= now < end_at` wins outright. On overlapping
   overrides, the **latest-created wins** — approximated by the greatest `OnCallOverrideId`, since
   ids are UUIDv7 (time-ordered), so no extra `created_at` column or query is needed to break the
   tie.
2. Otherwise, the highest-`priority` layer whose participant list is non-empty yields
   `layer_on_call`'s answer.
3. `None` if no override applies and every layer is empty (or there are no layers at all).

`layer_on_call` computes a **shift index** — the count of handoff boundaries between
`layer.anchor_at` (when `participants[0]`'s first shift started) and `now` — then indexes into
`participants` with that count modulo the participant count (`rem_euclid`, so it's well-defined
even for instants before the anchor). `next_boundary` answers "when does the current shift end",
used to build `OnCallNow.shiftEndsAt`/`nextUserId`.

**Mandated property tests** (via `proptest`, in `rim_core/src/oncall/resolve.rs`) assert
*invariants*, not a duplicate reference implementation:
- Monotonic within a shift: constant between consecutive handoff boundaries, changes only at them.
- Round-robin coverage: over one full cycle, each participant covers exactly one shift, in
  `position` order.
- Override precedence: an override covering `now` always wins, regardless of layer priority.
- Empty schedule always resolves to `None`.

Plus explicit unit tests for the two DST edge instants and the two full-week DST-boundary cases.

### `resolve_shifts` — coverage over a range

`resolve_shifts(snapshot, from, to) -> Vec<OnCallShift>` (`OnCallShift { user_id, start_at, end_at,
is_override }`) is `resolve_on_call` integrated over `[from, to)`, run-length-encoded into
contiguous blocks. It shares the exact same DST/override math — it's defined purely in terms of
`resolve_on_call` and boundary enumeration, never a re-derivation. Because layer priority and
participant emptiness are static for a given snapshot, exactly one rotation layer can ever be the
"base" winner across the whole range; only overrides transiently pre-empt it. The function walks
just that one layer's periodic boundaries plus every override's `start_at`/`end_at` inside the
range, sorts and dedups them, and calls `resolve_on_call` once per resulting segment (merging
adjacent segments that resolve to the same user **and** the same `is_override`, so a segment never
silently merges across an override boundary even if the user happens to coincide).

`is_override` (§10 FLAG B) records whether that segment's `user_id` came from an
[`OnCallOverride`] rather than the base rotation layer — computed per segment by checking whether
an override covers the segment's start instant, the same check `resolve_on_call` makes
internally. It exists purely so the frontend can render override segments distinctly (e.g. the
`RotationPreviewStrip`); it carries no resolution-order meaning of its own.

**Invariant, enforced by a property test:** for any `t` in `[from, to)`, the shift containing `t`
must name the same user as `resolve_on_call(snapshot, t)` called directly. The two must never
diverge — this is the mandated correctness tie between the two functions.

`resolve_shifts` itself has **no span cap** (a pure function shouldn't reject its own inputs); the
**service layer rejects a span over 90 days**, and (C1 security review) a `Custom` layer's
`interval_seconds` must be **`>= 300`** (`MIN_CUSTOM_INTERVAL_SECONDS`) — together these bound a
legitimate call to at most 90d / 300s = 25,920 boundaries. `resolve_shifts` additionally carries its
own hard backstop, `MAX_CUSTOM_BOUNDARIES` (50,000, with headroom above that legitimate worst
case): if a caller somehow bypasses both service-layer checks (e.g. a schedule persisted with a
tiny `interval_seconds` before the floor existed), boundary collection simply stops early rather
than walking an unbounded number of them — defense-in-depth *inside* the pure function, not a
replacement for the service-layer validation.

## GraphQL surface (C1a slice)

Schedule/rotation/override CRUD is gated on **Owner or Moderator** (`ensure_server_owner_or_moderator`
— per-channel moderator roles don't apply, since schedules are server-scoped; see iron-man's §10
ruling, applied identically to escalation policy CRUD in C1b). All read queries
(`onCallSchedules`/`whoIsOnCall`/`serverOnCall`/`onCallShifts`) are open to any workspace member.
`setRotationParticipants`/`createOnCallOverride` additionally validate every target `user_id` is a
member of the schedule's server, rejecting the whole call atomically if not (a cross-server user
could otherwise be put on-call) — a write-time-only check that does not self-heal if the user
later leaves the server (page-time already degrades gracefully, per `resolve_on_call`'s empty-layer
handling above).

```graphql
mutation {
  createOnCallSchedule(serverId: "...", name: "Primary", timezone: "America/Chicago") { id }
}

mutation {
  addRotationLayer(
    scheduleId: "..."
    name: "Primary rotation"
    priority: 0
    kind: { kind: WEEKLY, handoffWeekday: 0, handoffTime: "09:00:00" }
    anchorAt: "2026-01-05T09:00:00Z"
  ) { id }
}

mutation {
  setRotationParticipants(layerId: "...", userIds: ["<u1>", "<u2>"]) { id participants }
}

query {
  whoIsOnCall(scheduleId: "...") {
    currentUser { id username }
    shiftEndsAt
    nextUser { id username }
  }
}

query {
  # Batch across every schedule on a server — avoids an N+1 of individual whoIsOnCall calls for
  # a status-bar-style UI.
  serverOnCall(serverId: "...") {
    scheduleId
    currentUser { id username }
    shiftEndsAt
  }
}

query {
  onCallShifts(scheduleId: "...", from: "2026-01-01T00:00:00Z", to: "2026-01-08T00:00:00Z") {
    user { id username }
    startAt
    endAt
  }
}
```

`RotationKindInput`/`RotationLayer.kind` flatten the Rust `RotationKind` enum into a discriminator
(`kind: RotationKindGql`) plus optional per-kind fields (`handoffTime`, `handoffWeekday`,
`intervalSeconds`) — GraphQL has no native tagged-union input type, so this is the standard
"flattened union" shape (`EscalationTarget`'s eventual GraphQL input, C1c, will use the same
pattern). `handoffTime` is a plain `"HH:MM:SS"` string.

`OnCallNow`/`OnCallShift` resolve `currentUser`/`nextUser`/`user` as nested `User` objects via the
shared `User` DataLoader (the same pattern `Message.author` uses) — they don't expose bare user-id
fields at all, so a client never has to make a second round-trip or run its own N+1 to show a name.
`OnCallShift` additionally carries `isOverride: Boolean!` (§10 FLAG B) so the frontend can style
override segments distinctly from rotation segments.

`onCallShifts` rejects (`Validation`) an inverted range (`from >= to`) or a span over 90 days —
see `resolve_shifts`'s doc above for why the cap lives in the service layer.

`OnCallSchedule.rotationLayers: [RotationLayer!]!` is a nested `ComplexObject` field (not a
top-level query — matches how `Server.viewerRole`/`Incident.escalationRun` expose related data)
listing the schedule's existing layers, each with its participants attached, ordered `priority
DESC, created_at ASC`. It closes a C1 gap: `addRotationLayer`/`updateRotationLayer` let a client
create/edit a layer it already has the id for, but nothing exposed a schedule's *existing* layers,
so an editor UI could create rotations but never load one to edit — create-only, poor UX. Any
server member may read it (unlike the CRUD mutations, which are Owner+Moderator).

Full mutation/query list: `createOnCallSchedule`, `updateOnCallSchedule`, `deleteOnCallSchedule`,
`addRotationLayer`, `updateRotationLayer`, `removeRotationLayer`, `setRotationParticipants`,
`createOnCallOverride`, `deleteOnCallOverride`; queries `onCallSchedules(serverId)`,
`whoIsOnCall(scheduleId, at: DateTime)` (`at` defaults to now), `serverOnCall(serverId)`,
`onCallShifts(scheduleId, from: DateTime, to: DateTime)`.

## `user.timezone`

An optional `timezone` column was added to `user` (`m20260710_094331_add_user_timezone`),
**display/defaulting only** — it does not participate in any handoff math (see above). It exists
for (a) rendering on-call times in the viewer's local timezone on the frontend and (b) defaulting
a new schedule's timezone. No mutation exposes it yet in C1a; wiring a `setMyTimezone`-style
mutation is a small, separable follow-up whenever the frontend needs it.

## Escalation policies (Track C1b)

An **`EscalationPolicy`** (server-scoped) holds an ordered list of **`EscalationStep`**s. Each step
has a `delay_seconds` (wait before it fires, measured from the previous step firing — or from
incident declare for step 0; `0` fires immediately), a **target**
(`EscalationTarget::User(user_id)` or `EscalationTarget::OnCall(schedule_id)`, resolved via this
module's `resolve_on_call` at fire time — not at authoring time), and a non-empty set of
**actions** (`EscalationAction::MentionInChannel` and/or `::Email`; `Push` is a reserved-but-
unimplemented action kind for Track D4 — the enum slot exists so a future action doesn't need a
migration). A policy requires at least one step, and every step requires at least one action —
enforced by `EscalationPolicyService` at authoring time, not by the schema.

A **`conversation_escalation_policy`** row binds a policy as the default for incidents declared in
that conversation. `start_incident` accepts an optional per-call override; `ingest_alert` always
uses the channel default (there's no human to pick an override for an alert-declared incident).

**Targets are constrained to the policy's own server (C1 security review).** `validate_steps`
rejects, at authoring time (both `createEscalationPolicy` and `updateEscalationPolicy`), any
`User(u)` target where `u` is not a member of the policy's `server_id`, and any `OnCall(schedule)`
target whose schedule belongs to a *different* server — mirroring the `ensure_all_members` pattern
already used for rotation participants/overrides. Without this, an Owner/Moderator of one server
could author a step that pages a user (or resolves an on-call schedule) belonging to a different
server entirely, leaking that incident's title and channel link to them at fire time — a
cross-tenant paging hole caught in the C1 review. `DefaultIncidentService::perform_step_actions`
carries a second, page-time recheck as defense-in-depth (see "Fire" below): even a target that was
valid when the policy was authored is dropped if they've since left the server.

### Escalation wiring

`IncidentService` — not a separate escalation service — owns arming/cancelling/executing the
chain, because it already holds the incident lifecycle transitions that need to trigger it:

- **Arm on declare.** `start_incident`/`ingest_alert` resolve the applicable policy (override or
  channel default), and if one applies, create the `escalation_run` row **inside the same create
  transaction** as the incident. The scheduler is armed (`EscalationScheduler::arm`) only **after**
  that transaction commits — a rollback must never leave an in-memory timer armed with no matching
  DB row.
- **Cancel on ack/mitigate/resolve.** `acknowledge_incident`, `update_incident_status` (transitions
  to `Mitigated` or `Resolved`), and `resolve_incident` all cancel the incident's active
  escalation run (`EscalationRunRepository::set_status(.., Cancelled)` +
  `EscalationScheduler::cancel`). A `Mitigated → Active` reopen does **not** re-arm a cancelled
  run — reopening is rare and out of scope for C1b.
- **Fire.** The scheduler's driver task (in `rim_server::app::build_app_runtime`, mirroring the
  pipeline-evaluator background task) calls `IncidentService::fire_escalation_step(run_id)` when a
  timer elapses. That method: reloads the run and **no-ops if its status is no longer `Active`**
  (the guard against an ack/resolve race — §6's at-least-once semantics rely on this reload, not on
  the scheduler being race-free); resolves the step's target to concrete user id(s); **drops any
  resolved target who is no longer a member of the incident's server** (C1 security review —
  page-time defense-in-depth for `validate_steps`'s write-time constraint above; a target removed
  from the server after the step was authored, or after an on-call schedule/override last
  resolved them, is silently omitted rather than paged, exactly like an empty/deleted on-call
  schedule); performs each remaining action best-effort (a send failure is logged at `warn` and
  does **not** abort the remaining actions or the chain); appends an `incident_phase` timeline
  note; and advances to the next step
  (persisting `current_step_index`/`next_fire_at`, then re-arming) or marks the run `Completed` if
  there is no next step.
- **`incident.escalate`** (the builtin command) looks up the incident's active run and calls
  `fire_escalation_step` directly — "trigger the current step now" rather than waiting out the
  remaining delay.

### Acknowledgement

`acknowledged_at`/`acknowledged_by` are new nullable `Incident` fields — **not** a new
`IncidentStatus` variant, since ack means "a human has this, stop paging" and can coexist with
`Active`. `acknowledge_incident` requires only server membership (any member may ack, not
owner-gated — whoever responds picked it up) and is **idempotent**: acking an already-acked
incident returns the incident unchanged rather than erroring or re-cancelling.

### Escalation mention author

Until a seeded system/bot user exists (see "Follow-ups"), the escalation `MentionInChannel` action
is authored as the incident's `started_by` (always a real, already-a-server-member user —
`Principal::bot`/`webhook` carry `user_id: None` and can't author a message today). If that user
has since left the server, the mention post fails its member-check and is logged/swallowed (the
`Email` action, if also configured on the step, still sends).

### Timer wheel + boot recovery

`EscalationScheduler` (`rim_services::escalation::scheduler`) is a small trait — `arm(run_id,
fire_at)` / `cancel(run_id)`, both synchronous — behind which `InMemoryEscalationScheduler`
(`rim_infra_services::escalation`) is a single-node `DashMap<EscalationRunId, DateTime<Utc>>` plus
a `tokio::sync::Notify` so a newly-armed earlier deadline (or a cancel) wakes the driver task
immediately rather than waiting out a stale sleep. This matches the documented single-node stance
for in-memory infra elsewhere in this codebase (`fable-notes.md` §7: broadcaster, rate limiters —
"swap for Redis later, behind traits").

**The DB `escalation_run` row is the source of truth; the in-memory wheel is only the timing
mechanism.** `escalation_run` has a partial unique index on `(incident_id) WHERE status='active'`
(at most one active run per incident) and a partial index on `(status, next_fire_at) WHERE
status='active'` (the boot-recovery scan).

**Boot recovery** (`rim_infra_services::escalation::recover_active_escalation_runs`, called from
`build_app_runtime` before the driver task spawns) loads every `status='active'` row and arms it
from its persisted `next_fire_at` — including one already in the past, which the driver's
`sleep_until`-style loop then fires immediately (a missed page during downtime pages ASAP on
restart, which is the correct direction for an alerting system). This makes a node restart lose
nothing: the wheel is fully rebuilt from the DB.

**At-least-once paging** is the explicit, documented choice: every fire reloads the run and bails
if `status != Active` (the race guard described above), and advancing a step persists
`current_step_index`/`next_fire_at` in one update — so a crash between firing and persisting at
worst re-pages one step on recovery. That's the safe direction for an alerting system.

**Multi-node caveat** (mirrors `fable-notes.md` §7): two nodes would each independently arm and
could double-page the same run, or a node's restart could lose that node's armed timers entirely
(boot recovery covers the "whichever node restarts, reloads from DB" case, not true multi-node
coordination). This is an availability/UX regression under naive multi-node, not a security one —
the trait boundary is the swap point for a `SELECT ... FOR UPDATE SKIP LOCKED` poller or a
Redis-backed scheduler later.

`RIM_ESCALATION_TIMER_RESOLUTION_SECS` (default 5) is **only a floor** for the driver's idle sleep
when nothing is currently armed — the driver actually sleeps until the next armed deadline
(`sleep_until`-style), it does not poll on a fixed interval.

### Builtin commands (§10.1)

Four commands are registered (all `keybind_capable: true`; as of **C2 all four are now
`ai_callable: true`** — see the "C2 — AI agent operation" section below): `incident.ack` (Mutating,
calls `acknowledge_incident`), `incident.escalate` (Mutating, calls `fire_escalation_step` for the
active run), `oncall.who` (Safe, calls `who_is_on_call`), `oncall.take_shift` (Mutating, calls
`create_override` for the acting user — the caller supplies `start_at`/`end_at` explicitly;
there is no invented default "short" duration; note `create_override` is gated **Owner+Moderator**,
§10, so a plain member cannot take a shift). Each executor calls the exact same service method
its GraphQL mutation/query calls — unified dispatch, no duplicated logic.

### C2 — AI agent operation

_Spec: `docs/specs/track-c2-agent-operator.md`. The moat: everyone else's AI *suggests*; RIM's
*acts* under a human confirmation gate, and the incident timeline proves it._

The four lifecycle commands are **AI-callable** (`ai_callable: true`) with `nl_description` +
`intent_samples` for intent routing, so the client-side BYO-model agent can operate incidents.
This is a field/behavior upgrade on the C1 registrations — no re-architecture, no schema change.

- **Confirmation policy (no exemptions).** The gate is the existing
  `CommandDefinitionDto::requires_confirmation_for_initiator`: `oncall.who` is Safe and executes
  one-shot for any initiator including the agent; `oncall.take_shift`, `incident.escalate`, and
  `incident.ack` are Mutating, so an **agentic** initiator always gets a `ConfirmationRequired`
  challenge and must re-submit a verified `InBandToken` proof — a human approves every mutating
  agent action. Human initiators (Palette/Keybind/ContextMenu/Cli) are unchanged (one-shot); no C1
  surface behaves differently. `incident.ack` cancels the escalation chain and **stops paging** —
  the exact class of action the human gate exists for — and is deliberately **not** exempted
  (ratified by Jakub).
- **Ack consequence copy.** `incident.ack`'s `description` states the paging-stop consequence
  ("…cancels the escalation chain and stops paging on-call…"); the FE agent-confirmation modal
  renders `Command.description` so the approver sees it before confirming. (No challenge-message
  API field — the copy is static per command; see spec §4.)
- **Agent-action transcript.** When an agentic initiator runs `incident.ack` or `incident.escalate`,
  the service appends an `incident_phase` timeline note ("… via AI agent (initiator: …)") — gated on
  `is_agentic()`, so human invocations and direct GraphQL mutations write no note, and it fires
  exactly once per action (never on the idempotent already-acked path or a no-active-run escalate).
  `oncall.who` (read) and `oncall.take_shift` (schedule-scoped, no incident) write no note; the
  latter is still fully audited via the command audit log.
- **Capabilities = `[]` (intentional).** The four keep `required_capabilities: []`. Builtin caps
  have no per-user grant store today (the check is registry-lookup + server-membership), so seeding
  a capability would only duplicate the `ensure_server_member` the service methods already enforce
  while adding a `workspace_id`-context precondition that breaks arg-only agent calls and the C1
  paged-toast/keybind ack. AI-surface *visibility* is not *authorization*: every one of the four is
  authz-gated at the service layer (`ensure_server_member`; `oncall.take_shift` is stricter —
  Owner+Moderator via `create_override`). **Revisit** if/when a real per-user capability grant store
  exists (Track D3 / plugin runbooks).
- **Composes into agent runbooks for free.** Flipping `ai_callable` makes these legal steps in
  agent-routed macros; the existing macro sensitivity rollup (`CommandSensitivityDto::effective` =
  max, per-step confirmation-token issuance) gates a runbook by its worst step — no new rollup code.

### GraphQL surface (C1c slice)

Policy CRUD — `createEscalationPolicy(serverId, name, steps)`, `updateEscalationPolicy(policyId,
name, steps)`, `deleteEscalationPolicy(policyId)`, `setChannelEscalationPolicy(serverId,
conversationId, policyId)` — is gated **Owner+Moderator**, same as schedule/rotation/override
editing (§10 ruling). Every step's target is validated to belong to the policy's own server (the
C1 security-review fix — see `EscalationPolicyService::validate_steps`'s doc comment). The
`escalationPolicies(serverId)` query is open to any workspace member.

`EscalationStep`/`EscalationTargetInput` use the same flattened-discriminator pattern as
`RotationLayer`/`RotationKindInput`: `targetKind`/`kind` selects between `targetUserId`/`userId`
and `targetScheduleId`/`scheduleId`, with the same strict cross-field rejection (a `userId` set
alongside `kind: ON_CALL` is a `Validation` error, not silently ignored).

`acknowledgeIncident(incidentId): Incident!` requires only server membership — any member may
ack. `startIncident` additionally accepts an optional `escalationPolicyId` to override the
conversation's default policy for that one incident (§3).

`Incident` gains `escalationPolicyId`, `acknowledgedAt`, `acknowledgedBy` (bare id) and
`acknowledgedByUser: User` (resolved via the shared `User` DataLoader, the `message.author`
pattern), plus `escalationRun: IncidentEscalationRun` (FLAG C) — a nullable read projection of
the active `escalation_run` row (`currentStepIndex`, `nextFireAt`, `status`) so a client loading
mid-incident, or reconnecting, can render a live "escalating to L2 in 4:32" countdown without
waiting for the next `EscalationStepFired` event. `null` when no policy is attached or the run
has already completed/been cancelled. Backed by a new `IncidentService::
get_active_escalation_run` method — deliberately takes no `Actor`, since its only caller (this
resolver) has already loaded, and therefore permission-gated, the parent `Incident` object.

## Follow-ups (explicitly out of scope for C1)

- Seeded system/bot user for cleaner escalation-post attribution (today: authored as the
  incident's `started_by`).
- Per-participant follow-the-sun timezones (restricting a participant to their working hours).
- `Webhook` escalation target (enum slot reserved, not implemented).
- `Push` escalation action (Track D4; enum slot reserved).
- Repeat/re-escalation loops, and escalating to "the next" on-call after the current one.
- Schedule ICS import/export.
- A `setMyTimezone`-style mutation for `user.timezone` (currently write-only via direct DB access
  in tests; no GraphQL mutation exposes it yet).
