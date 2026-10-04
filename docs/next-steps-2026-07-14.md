# RIM — Next Steps (handoff for a fresh session)

_Written 2026-07-14 after committing Track C2. Supersedes `docs/next-steps-2026-07.md`
(still accurate for A/B/C1 context). Start here._

## Where we are

Tracks **A** (foundations), **B** (search / alert-ingestion / email), **C1** (on-call +
escalation), and **C2** (the agent-operator moat) are **complete, reviewed, and committed**:

- rim-server: `15369fb` (C2), `f68ab3c` (security) on `feature/10-lets-start-over`
- rim-ui: `d907c86` (C2) on `feature/initial-code`
- **Not pushed** — local commits only, awaiting your review/push.

The moat is now real end-to-end: the BYO-model agent can **operate incidents** — ack, escalate,
query on-call, take a shift — each mutating action behind a human confirmation proof, with a
"via AI agent" transcript note in the incident timeline. Everyone else's AI suggests; RIM's acts
under a gate. Full C2 detail: `docs/specs/track-c2-agent-operator.md`, `docs/oncall.md` §"C2 — AI
agent operation". C2 was a thin fusion layer — the confirmation gate, AI-surface filter, and macro
sensitivity rollup all pre-existed.

## Recommended next: Track D2 — Agentic runbooks (the moat's payoff)

Per the ratified roadmap, **D2 = a runbook (macro/pipeline of commands) the agent walks
step-by-step during an incident**, each Mutating/Dangerous step through the confirmation flow, the
execution transcript landing in the incident timeline. This is the flagship demo: a responder says
"run the DB-failover runbook," the agent walks it, pausing for a human OK on each dangerous step,
and the whole run is recorded on the incident. **The infra is ready** — the macro sensitivity
rollup (`effective()` = max), per-step confirmation-token issuance (`macro_def.rs`), and the C2
`is_agentic()` timeline-note pattern already exist. D2 is the payoff on top, not new plumbing.

**Key design questions for iron-man/vision to rule on (spec these first):**
1. **Runbook = existing macro, or a new first-class object?** Recommendation: start with
   macro-as-runbook (macros are already `ai_callable` + rolled-up) + an incident association at
   execution time; only promote to a dedicated `Runbook` object if binding/versioning demands it.
2. **Stepwise execution vs whole-macro execution.** Today `execute_macro` runs all steps with one
   outer proof + per-step tokens. D2's differentiator is the agent **pausing at each mutating step
   for a fresh human decision** with visible progress. Decide: does the agent drive the macro
   step-by-step via individual command calls (it already can — it has the tool list), or do we add
   a "stepwise macro execution" API (run step N, return, await confirmation, run N+1)? This is the
   core new mechanism — get the architect's ruling before building.
3. **Transcript to the incident timeline.** Extend the C2 pattern: each executed step appends an
   `incident_phase` note ("Runbook 'DB failover' step 3/5: restart-replica — via AI agent"). Reuse
   the `is_agentic()`-gated note hook C2 added in `incident.rs`.
4. **Incident binding.** A runbook run needs an incident context so its transcript lands on the
   right timeline — a new arg/context on the execution path.
5. **Frontend (the larger half this time, unlike C2):** a runbook-execution view — step progress,
   inline pending confirmations, the running transcript. vision to scope.

**Effort: M/L.** Land in reviewable milestones (D2a stepwise mechanism + transcript backend; D2b
FE execution view). Same flow: spec → implement → architect fit-review → hawkeye + black-widow →
independent verification → DoD.

### Alternatives (if you'd rather ship parity first)
- **C5 — Incident metrics** (fastest win): MTTA/MTTR from existing incident + ack timestamps, plus
  a history/archive view (the backend `listIncidents` query already shipped). Small, self-contained,
  closes a visible parity gap. Good if you want a quick S-effort session.
- **C4 — Grounded cited recall** (defense/parity vs Mattermost Agents V2): cited answers over the
  workspace via the BYO model + FTS (B1, shipped). Medium. Not a differentiator — catch-up.
- **C3 — AI postmortem v2**: draft from the tagged timeline with citations; action items →
  trackable objects. Parity-plus.

**My recommendation:** D2 next — it makes the moat *demonstrable* and the infra is ready, so it's
high-leverage/low-risk. C5 is the fallback if you want a quick parity win instead. Confirm or
redirect at session start.

## Open decisions (Jakub)

- **`oncall.take_shift` authorization (surfaced by C2).** `create_override` is gated
  **Owner+Moderator** (overrides are schedule edits), but C1 framed take_shift as "self-service." So
  a plain member's agent request to take a shift is denied. C2 made the `nl_description` honest about
  this. **Your call:** should take_shift be genuinely self-service (any member can override *their
  own* on-call), or stay owner/mod-gated? Affects D2 runbooks that include it.
- **`.env` history purge** — rotation + untrack are done (leaked secret is dead); the git-filter-repo
  history scrub + force-push to shared `master` is **not** done (too risky unattended). Runbook:
  `docs/security/env-history-purge-runbook.md`. Run it when you're ready, or decide to skip (the
  exposure is already neutralized by rotation).
- **Audit retention** (Variant A 90d/365d vs B 365/365) — still unratified; the retention job is
  unscheduled (`delete_older_than` exists, nothing calls it). Carryover.
- **Mobile paging scope** (Track D4): PWA vs native. C1/C2 reachability is in-app only.

## Backlog (low priority, all tracked)

- seeded system/bot user — proper escalation/agent-action author (today: `started_by` / raw user_id
  in notes); unblocks `Principal::bot/webhook` authoring.
- serde derives on `Principal` + `SkillTier` — unblocks `--all-features`.
- channel escalation-policy binding READ query (`setChannelEscalationPolicy` is set-only).
- broader `DefaultEscalationPolicyService` test coverage.
- conversation_type on the six un-skipped WorkspaceEvent TS types.
- `find_current_phase().await?` in the C1/C2 timeline-note blocks propagates a phase-read error;
  hawkeye judged it intentional (matches precedent). If ever made best-effort, do it uniformly
  (C1 + C2), not just the C2 blocks.

## Environment (for the new session)

- **Toolchain**: `rust-toolchain.toml` pins 1.96; `Cargo.lock` committed — don't regenerate.
- **Postgres**: docker container `rim-postgres` (PG17, `:5432`, db `testdb`). Start it:
  `docker start rim-postgres`. `DATABASE_URL` is in `.env` (untracked now — still on disk).
- **DB/e2e tests**: `export $(grep DATABASE_URL .env)` then `--test-threads=1` with feature flags
  `db-tests` / `e2e-tests` / `infra-db-tests` (they PANIC if the flag is set without DATABASE_URL —
  by design).
- **SDL → codegen** (only when GraphQL changes; C2 needed none): `cargo run -p rim_server`, then
  `curl -s localhost:8080/api/sdl > ../rim-ui/schema.graphql`, then `npm run codegen`. Kill stray
  `:8080` binders first.
- **Agent team** (`~/.claude/agents/`): nick-fury (coordinator), **iron-man/vision architects — now
  `model: opus`** (switched from fable 2026-07-13; fable was a poor fit for design), hulk/spider-man
  programmers (sonnet), hawkeye/black-widow reviewers (opus), doctor-strange research (opus). Flow:
  architect spec → programmer to DoD → architect fit-review → hawkeye + black-widow gate → main
  session re-runs gates independently. Treat WS event shapes as frozen cross-repo contracts.

## Known state / hygiene

- Both repos DoD-green as committed: backend fmt / clippy -D warnings / 558 lib + db/e2e/infra-db/
  migration suites; frontend build / 1348 tests / lint (0 err) / format.
- **`.env` is now untracked** (working file on disk, gitignored, secret rotated). See the security
  runbook for the optional history scrub.
- `rim-server/target` is large; `cargo clean` is safe now that C2 is committed if disk is tight.
- Verification caveat: C2's server-side agent path is proven by integration + e2e tests; a true UI
  drive of the client-side BYO-model agent needs a live LLM API key (not runnable unattended).
