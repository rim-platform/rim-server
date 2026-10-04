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

## Plan

Superseded the same day: Jakub reset the direction from the incident wedge to a programmable
communication workspace that is not a Discord or Slack replacement, with end-to-end encryption
and decentralization as committed horizons. The plan now lives in `ROADMAP.md` (rebuilt
2026-10-04). Track 0 (desktop RCE fixes, terminal decision, repo hygiene, optional incident
module) is the first job; Track 2 (plugin SDK, the Kubernetes pods panel, web-capable and
workspace-shared plugins) is the main investment; Track 1 is a bounded "enough chat" list. The
"Known debt" list above is folded into the roadmap's debt section.

## Decisions needed (Jakub)

See `ROADMAP.md` "Open Decisions" for the full list (terminal, incidents-as-plugin, web plugin runtime, E2EE scope,
decentralization model, roles depth, mobile). Carried items:

1. **`oncall.take_shift` authorization**: genuinely self-service for one's own shift, or stay
   Owner/Moderator gated. Affects agentic macros that include it.
2. **Audit retention**: Variant A (90d/365d) vs B (365/365). Unblocks scheduling the job.
3. **`.env` history purge**: run the runbook (force-push to `master`) or skip, given rotation.

## Environment notes

- Toolchain pinned to Rust 1.96 by `rust-toolchain.toml`; `Cargo.lock` committed, do not
  regenerate.
- Integration suites need Postgres 17 and `DATABASE_URL`; run with `--test-threads=1` and the
  `db-tests` / `e2e-tests` / `infra-db-tests` features (they panic without `DATABASE_URL`, by
  design).
- After any GraphQL change: `cargo run -p rim_server`, then export `/api/sdl` into
  `rim-ui/schema.graphql` and run codegen there.
