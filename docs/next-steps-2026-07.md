# RIM — Next Steps (handoff for a fresh session)

_Written 2026-07-12 after committing Tracks A + B + C1. Start here._

## Where we are

Tracks **A** (foundations), **B** (full-text search, alert ingestion, email), and
**C1** (on-call schedules + escalation — the incident wedge) are **complete, reviewed,
and committed**:

- rim-server: commit `48b6eaf` on `feature/10-lets-start-over`
- rim-ui: commit `9ea67d5` on `feature/initial-code`

The incident wedge works end to end: **alert → incident → on-call resolution →
escalation timer chain → page (mention + email) → ack**, surviving a server restart.
Every piece went spec → implement → architect fit-review → hawkeye + black-widow →
independent verification. Backend: workspace tests + all feature-gated db/e2e suites
green. Frontend: 158 files / 1328 tests, build/lint/format clean, 0 prod vulns.

Full narrative + the strategic thesis: `ROADMAP.md` (Track C is RATIFIED + re-scoped
per `docs/research/mattermost-playbooks-teardown-2026-07-10.md`). Architecture review:
`fable-notes.md` in each repo.

## Recommended next: Track C2 — the moat

Per the ratified roadmap, **C2 = incident lifecycle-as-commands fused with the Track D
operator.** The four C1 commands (`incident.ack`/`incident.escalate`, `oncall.who`/
`oncall.take_shift`) already exist as plain keybind-capable builtins with `ai_callable=
false`. C2 is a pure upgrade on top: flip `ai_callable=true`, add `nl_description`/
`intent_samples` for intent routing, and layer the agent sensitivity-rollup +
confirmation flow — so the BYO-model agent can *operate* incidents (declare, page,
ack, run runbooks) under the existing human-gate. This is the differentiator no
competitor has: everyone else's AI *suggests*; RIM's *acts* under a confirmation gate.
Keep C2 tightly coupled to Track D (the agent operator) — the fusion is the moat, not
lifecycle commands alone.

## Backlog (low priority, all tracked)

- conversation_type on the six un-skipped WorkspaceEvent TS types (keep TypingStarted
  OFF — it's skip_serializing)
- seeded system/bot user — proper escalation-message author; also unblocks
  Principal::bot/webhook message authoring (`user_id` is None today)
- serde derives on `Principal` + `SkillTier` — unblocks `--all-features`
- broader `DefaultEscalationPolicyService` test coverage (only cross-server tests exist)
- channel escalation-policy binding READ query (`setChannelEscalationPolicy` is set-only
  today; UI can't display current bindings)

## Open decisions (Jakub)

- **Audit retention** (Variant A 90d/365d vs B 365/365) — unratified; the retention job
  is unscheduled (`delete_older_than` exists but nothing calls it).
- **Mobile paging scope** (Track D4): PWA vs native. C1 reachability is IN-APP only
  (banner, paged-toast, ack) — real device push is Track D4.
- Plugin client-side reframe ratification (Phase 5 carryover).

## Environment (for the new session)

- **Toolchain**: `rust-toolchain.toml` pins 1.96; `Cargo.lock` is committed. Don't
  regenerate the lockfile.
- **Postgres**: docker container `rim-postgres` (PG17, `:5432`, db `testdb`).
  `DATABASE_URL` is in `.env`.
- **DB/e2e tests**: `export $(grep DATABASE_URL .env)` then run with
  `--test-threads=1` and the feature flags `db-tests` / `infra-db-tests` / `e2e-tests`
  (they PANIC if the flag is on without DATABASE_URL — by design). A migration-guard
  test now fails loudly if `sea migrate generate` ever drops a registration.
- **SDL → codegen** (when GraphQL changes): run `cargo run -p rim_server`, then
  `curl -s localhost:8080/api/sdl > ../rim-ui/schema.graphql`, then `npm run codegen`
  in rim-ui. Only ONE rim_server should bind `:8080` — kill stragglers first.
- **Agent team** (`~/.claude/agents/`): nick-fury (coordinator), iron-man/vision
  (architects, model `fable`), hulk/spider-man (programmers, `sonnet`), hawkeye/
  black-widow (reviewers, `opus`), doctor-strange (research). Flow: architect writes a
  spec file → programmer implements to DoD → architect fit-review → hawkeye+black-widow
  gate → main session re-runs gates. Treat WS event shapes as frozen cross-repo
  contracts (JSON strings, no codegen safety).

## Known issues / hygiene

- **SECURITY (pre-existing):** `.env` is **tracked** in rim-server git history and
  contains real secrets (`RIM_BOOTSTRAP_SECRET`, DB creds). It is now gitignored so it
  won't be re-committed, but it's already in history — consider purging it
  (`git filter-repo`) and rotating `RIM_BOOTSTRAP_SECRET`. NOT addressed in the A/B/C1
  commit.
- `rim-server/target` is ~36G. Now that the work is committed, `cargo clean` is safe if
  the disk needs it. (Disk was chronically near-full this run; `~/.cache/yay` was
  cleared for ~29G.)
- `cargo-audit` upgraded to 0.22.2; config at `.cargo/audit.toml` ignores
  RUSTSEC-2023-0071 (`rsa`, reachable only via unused `sqlx-mysql`) — justified.
- The `--all-features` build fails only on the tracked `Principal`/`SkillTier` serde
  gap above; the CI feature-gated clippy step uses an explicit feature list to avoid it.
