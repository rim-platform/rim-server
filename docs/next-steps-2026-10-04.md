# RIM — Next Steps (handoff, 2026-10-04)

_Written 2026-10-04, the day PR #11 (`feature/10-lets-start-over`) merged into `master`.
Supersedes `docs/next-steps-2026-07-14.md` as the entry point; that file is still the accurate
narrative for Tracks A/B/C1/C2. Start here._

## Where we are (verified against code and GitHub, not just the docs)

**`master` now carries the whole rebuild.** PR #11 merged today at `0c227ca` with Tracks A
(foundations), B (full-text search, alert ingestion, email), C1 (on-call + escalation) and C2
(agent-operator fusion), plus two security rounds (July: `.env` untracked; October: S-H2, S-H3,
S-M1..S-M5, S-L5 from `fable-findings.md`) and a CI rework (single instrumented build, parallel
jobs, coverage gate temporarily 75%, measured 77.3%). CI was green on the PR's final commit;
the first `master` run (#16) was still in progress when this was written.

Repository state:
- No open pull requests. Open issues #4, #9, #10 are stale (#10 is the one PR #11 closed out).
- `ROADMAP.md` is current on strategy but its "Where We Are Now" ledger is from 2026-07-08 and
  predates everything above. `fable-findings.md` has a status table re-checked on 2026-10-04.
- The frontend (`rim-ui`) was not available in this session; its state is as described in the
  July handoff (C2 committed at `d907c86` on `feature/initial-code`).

Checked in code today (claims in the older docs that are now confirmed true):
- `mocks` feature is dev-dependency only in `rim_api`, `rim_server`, `rim_infra_services`.
- `rim_server/src/app.rs` builds repos via `AppRepos::with_handle` (A-M8 closed).
- `command.discover` exists (`rim_infra_services/src/command/discover.rs`) and
  `synthesize_command_definitions` exists, so the Phase 5 ship gate is in the tree.
- Password reset revokes all sessions in one transaction (S-M1).
- 59 migrations; roughly 1,180 test functions across the workspace; db/e2e/infra-db suites
  behind feature flags with loud guards.
- `cargo check --workspace --all-targets` passes on a fresh clone with the pinned 1.96.1
  toolchain (checked today).

Still open from the reviews (confirmed still true today):
- **No through-the-stack messaging e2e** (`rim_api/tests` has only registration and password
  reset). The `BEFORE` paginator was reworked but never re-verified end to end (A-H1/A-M7).
- **God files grew**: `rim_api/src/types.rs` 3,694 lines, `mutation.rs` 2,846, `query.rs` 1,543.
- **Audit retention is unscheduled**: `delete_older_than` has no production caller.
- **`rim_services/src/plugin_host`** (manifest/runtime/validation) is still dead code on the
  server side.
- `cargo audit` in CI is `continue-on-error`, so the dependency gate is advisory only.
- Coverage gate at 75% with 95% as the stated target.
- `.env` history purge not done (runbook in `docs/security/env-history-purge-runbook.md`);
  the leaked secret was rotated, so exposure is neutralised.

## Recommended plan

The strategy in `ROADMAP.md` stands: incident wedge as the story, programmable runtime plus the
confirmation-gated operator as the moat. The next milestone is the one the July handoff already
pointed at, with a short housekeeping step first.

### Step 0 — Housekeeping (this week, S)

1. Watch `master` CI run #16 to green; fix anything that only fails on `push` (none expected).
2. Refresh the `ROADMAP.md` "Where We Are Now" ledger to this document's state, mark Tracks
   A/B/C1/C2 shipped, and point "Known debt" at the open list above.
3. Close issues #10 (done by PR #11) and the stale #4/#9, or convert them into real tickets.
4. Make `cargo audit` blocking in CI (the `.cargo/audit.toml` ignore for RUSTSEC-2023-0071 is
   already justified, so it should pass).
5. Decide go/no-go on the `.env` history purge (see decisions below).

### Step 1 — Track D2: Agentic runbooks (the flagship, M/L)

This is the payoff on top of C2 and the demo the whole thesis rests on: a responder says
"run the DB-failover runbook", the agent walks it step by step, pauses for a human OK on each
Mutating/Dangerous step, and the run lands in the incident timeline. The plumbing exists
(macro sensitivity rollup, per-step confirmation tokens in `macro_def.rs`, the C2
`is_agentic()` timeline-note hook in `incident.rs`).

Land it in two reviewable milestones:
- **D2a (backend, this repo):** stepwise macro execution API (run step N, return, await
  confirmation, run N+1), an incident binding on the execution path, one timeline note per
  executed step, audit chain parent/child per step, integration tests under
  `infra-db-tests`, SDL change documented for codegen.
- **D2b (frontend, rim-ui):** runbook execution view with step progress, inline pending
  confirmations and the running transcript.

Spec first. The two design rulings that gate D2a are listed under decisions.

### Step 2 — Parity quick wins (S each, fill-in work alongside D2)

- **C5 incident metrics**: MTTA/MTTR from existing incident and ack timestamps plus the
  history/archive view (`listIncidents` already shipped).
- Channel escalation-policy binding READ query (`setChannelEscalationPolicy` is set-only).
- `setMyTimezone` mutation for `user.timezone` (today write-only via DB).
- Seeded system/bot user as the author of escalation and agent actions (unblocks
  `Principal::bot/webhook` authoring).

### Step 3 — Quality debt, interleaved one item per milestone

- Messaging e2e under `e2e-tests` that walks send/reply/paginate both directions through
  GraphQL; this is what finally verifies the paginator fix.
- Split `mutation.rs`/`query.rs`/`types.rs` by domain with `MergedObject`; do it before D2a
  adds more resolvers.
- Schedule the audit retention job once the variant is ratified.
- Delete or relocate the dead `rim_services/plugin_host` module.
- Raise the coverage gate in steps (80, 85, ...) as the e2e suites land.

### After that (unchanged order from the roadmap)

C4 grounded cited recall (parity with Mattermost Agents V2), C3 AI postmortem v2, D3 runbook
plugins (needs the plugin-host install hardening from Track A6), D4 on-call reachability
(blocked on the mobile decision), then Track E (CLI v1, MCP bridge).

## Decisions needed (Jakub)

1. **D2 runbook object**: reuse macros as runbooks with an incident binding at execution time
   (recommended), or a new first-class `Runbook` object.
2. **D2 execution model**: add a stepwise macro execution API (recommended, it is the
   differentiator), or let the agent drive steps as individual command calls.
3. **`oncall.take_shift` authorization**: genuinely self-service for one's own shift, or stay
   Owner/Moderator gated. Affects runbooks that include it.
4. **Audit retention**: Variant A (90d/365d) vs B (365/365). Unblocks scheduling the job.
5. **Mobile scope** (Track D4): PWA-first (recommended) vs native.
6. **`.env` history purge**: run the runbook (force-push to `master`) or skip, given rotation.

## Environment notes

- Toolchain pinned to Rust 1.96 by `rust-toolchain.toml`; `Cargo.lock` committed, do not
  regenerate.
- Integration suites need Postgres 17 and `DATABASE_URL`; run with `--test-threads=1` and the
  `db-tests` / `e2e-tests` / `infra-db-tests` features (they panic without `DATABASE_URL`, by
  design).
- After any GraphQL change: `cargo run -p rim_server`, then export `/api/sdl` into
  `rim-ui/schema.graphql` and run codegen there.
