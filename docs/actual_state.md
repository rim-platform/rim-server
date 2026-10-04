# RIM — Actual State Inventory (2026-04-25)

> **Note (2026-10-04):** the server-side `rim_services::plugin_host` module discussed below was removed on 2026-10-04 as unused. The plugin host and its manifest validation live in the desktop client (`rim-ui/src-tauri/src/plugin_host/`).

Honest snapshot of what's truly shipped vs claimed shipped, cross-checked against code state and Hawkeye verdicts. Distinct from `ROADMAP.md` (aspirational) and the team task ledger (which has been overstated by leads claiming "complete" before review).

**Legend:**
- ✅ shipped & working end-to-end
- 🟡 works with gaps
- 🔧 in-flight (Phase 5)
- 📦 scaffolded but unwired / dead code
- ❌ planned, not started

---

## 1. Communication Core (Phase 1) — ✅ Complete

The chat product foundations are all there.

- ✅ Workspaces / servers (multi-tenant)
- ✅ Channels (public, private), DMs, group DMs
- ✅ Threads, replies
- ✅ Message attachments (file uploads)
- ✅ Mentions (@user, @channel)
- ✅ Reactions (emoji-mart v5 picker, full Unicode + skin tones)
- ✅ Pinning, message forwarding (`forwarded_from_message_id` on `NewMessage`)
- ✅ Typing indicators
- ✅ Read state per channel
- ✅ Message search baseline
- ✅ Pagination (cursor-based, standardized `(timestamp, id)` tuples per `docs/pagination.md`)
- ✅ Permalinks
- ✅ Custom emoji
- ✅ Link previews
- ✅ Permissions / membership rules

---

## 2. Command System (Phase 2) — ✅ Complete

The programmability spine.

- ✅ Built-in command registry (~30+ commands across chat, palette, navigation, admin, workspace state)
- ✅ Command palette UI
- ✅ Typed command arguments with validation
- ✅ User-defined custom commands (saved per workspace)
- ✅ Chained command piping with pre-validation pass + per-stage results + explicit `isPipe` flag
- ✅ Command execution context (selection, channel, message refs)
- ✅ Macro autocomplete in composer with name + arg preview
- ✅ Macro name-to-ID resolution (GraphQL query + frontend async lookup)
- ✅ Dangerous-command confirmation modal
- ✅ Pipeline result view with collapsible JSON

---

## 3. Keybind System (Phase 3) — ✅ Complete

- ✅ 19 default keybinds shipped
- ✅ User-customizable per workspace
- ✅ Categories, context-aware
- ✅ Inline editing in Settings

---

## 4. Plugin System (Phase 4 + ongoing) — 🟡 Shipped with gaps

**Where it's solid:**
- ✅ Plugin manifest format + validation (N1 capability key validator, F2 install-ID leak walker — wired via task #11)
- ✅ Local plugin host lifecycle (Tauri-side — 18 modules in `rim-ui/src-tauri/src/plugin_host/`: broker, handshake, process, lifecycle, panel registry, panel relay, etc.)
- ✅ Install / enable / disable / uninstall via git-backed install
- ✅ Sandboxed iframe panels
- ✅ Context actions from plugin manifests
- ✅ Capability prompts at install
- ✅ Plugin state persisted to atomic-write `plugin_state.json`

**Where the gaps are:**
- 🟡 **Plugin AI consent** (Phase 5 #5): backend gates work, but `plugin_run_ai_command` IPC dispatch is currently a stub returning `PluginNotRunning` (broker-loop wiring deferred). Hawkeye also caught a frontend↔Rust `PluginInfo` schema drift — TS reads `plugin.enabled` and `plugin.manifest.commands` but Rust returns flat `status` + `approved_ai_commands` shape, so `injectPluginTools` is dead code in production. Pete is mid-fix.
- 📦 **Backend `plugin_host` module** (`rim-server/rim_services/src/plugin_host/`): 1,290 LOC of DTOs + validators that are NOT consumed anywhere in `rim_api` or `rim_infra_services`. Contracts-only. The actual plugin pipeline runs entirely in the Tauri client. Server-side plugin awareness doesn't exist yet. *(Removed 2026-10-04.)*
- 📦 Plugin runtime DTOs (`LocalPluginHostHandshakeRequest/Response`, `LocalPluginExecuteCommandRequest/Response`) — defined, never used. Aspirational scaffolding for future server-mediated plugin execution.

---

## 5. AI Agent — 🔧 Mid-reframe

Active work area. Status is messy because Phase 5 is mid-execution and the team ledger has been overstated by Strange.

### Truly shipped (per code + tests)

- ✅ **Reframe Phase 1 (BE foundation, 2026-04-22)**: `CommandDefinitionDto` carries `nl_description`, `intent_samples`, `ai_callable`, `sensitivity` (`Safe | Notable | Standard | Mutating | Dangerous`); `sanitize_nl_text` module; `CapabilityService::check()` real implementation with strict default-deny on unknown caps; `ConfirmationProof { InBandToken | OutOfBand }` with 5s TTL + server-recomputed args_hash + single-use + cross-command replay block; `CommandInitiator` enum on every execute path; `SENSITIVE_ARG_NAME_RE` + value-shape detectors.
- ✅ **Reframe Phase 4 (FE deletion + IPC, 2026-04-24)**: 13 hardcoded TS tools deleted from `rim-ui/src/features/agent/tools/builtins/`; agent now routes through unified `commandDispatcher.execute(...)`; Tauri IPC `agent_chat_completion` proxy with CSP webview-origin block; `tracing::info!(target = "audit", ...)` audit emission; `Zeroizing<String>` keys with `ZeroizeOnDrop`; `OnceLock<Client>` shared sidecar HTTP client; `ValidationOutcome` typed enum; 902/902 frontend + 12/12 agent_proxy tests.
- ✅ **Capability registry** SeaORM table with built-in caps seeded at startup.
- ✅ **Audit log foundation** (#1): `command_audit_log` Postgres table, dual-path write, capability-gated `list_entries` with privilege-escalation regression test (Widow-grade fix).
- ✅ **Keyring Option B** (#7): Tauri-side `KEYRING_SERVICE` constant, `set_provider_key(Zeroizing<String>)`, audit emit on missing-key reads, `ChatCompletionRequest` with `#[serde(deny_unknown_fields)]`.
- ✅ **F2 manifest validators wire-in** (#11): plugin discovery rejects tampered manifests; install-ID leak walker live.
- ✅ **Rate limit primitive** (#9 partial): sliding-window arithmetic, TTL eviction, per-actor + per-command isolation, all unit-tested.

### Claimed shipped but actually RED per Hawkeye verdicts in current session

- 🔧 **#2 typed_verb (BE)**: marked completed in ledger, but as of last Hawkeye pass: `db-tests` build broken (missing `uuid` dev-dep + 5 `AuditLogListQuery` literals missing `typed_verb: None`), required typed_verb integration test never written, no rate-limit e2e test, no GraphQL `RATE_LIMITED` extension shape test. Strange has the remediation list.
- 🔧 **#9 rate limits** (combined with #2): primitive works; the gate is wired into `execute_command` after capability/confirmation/before dispatch; but no e2e test proves the gate fires through `DefaultCommandService::execute_command`. Every builtin has `rate_limit_per_minute: None` so the live registry never exercises the path.
- 🔧 **#4 Skill Macros AI exposure**: marked completed in ledger, Hawkeye returned **6 BLOCKs**: missing migration (live DB writes will crash on missing columns), no parent/child audit chain, no `synthesize_command_definitions` discovery surface (the actual product closure), no sensitivity max-rule, `confirmation` field accepted then silently dropped, zero integration tests against spec scenarios. Reverted to in_progress.
- 🔧 **#5 Plugin AI consent (REFRAMED Tauri)**: in_progress. Pete shipped the gates + atomic save + audit emit + tests, but Hawkeye caught the frontend↔Rust `PluginInfo` schema drift. Pete remediating.

### Not yet started

- ❌ **#3 typed_verb (FE)**: blocked by #2
- ❌ **#6 Plugin consent UI**: blocked by #5
- ❌ **#8 Keyring FE + verification gate**: blocked by #7 (now unblocked but paused)
- ❌ **#10 `command.discover` (SHIP GATE)**: literal substring + token-overlap ranking; embeddings deferred. Blocked by #2, #4, #5, #9. **This is the actual Phase 5 ship gate.**

### The agent today (production)

- ✅ Calls commands via the unified registry through `commandDispatcher.execute`
- ✅ Tool-calls supported in the conversational UI
- ✅ Anthropic + OpenAI provider support via Tauri IPC proxy (CORS-blocked browser fetches eliminated)
- ✅ Audit log line emitted on every agentic invocation (`event=command_invoked initiator=agent`)
- 🟡 Cannot discover macros yet (#10 not shipped)
- 🟡 Cannot invoke plugin AI commands end-to-end (#5 dispatch stubbed + schema drift)

---

## 6. Workspace State — ✅ Complete

Programmable runtime KV store.

- ✅ Per-server / per-user scoping
- ✅ JSONB values
- ✅ TTL support with expiry
- ✅ Atomic increment
- ✅ Quota enforcement
- ✅ Real-time events via WebSocket
- ✅ Settings → State tab UI: prefix search, scope toggle, JSON validation, TTL input

---

## 7. Power-User UX — ✅ Complete

- ✅ **Zen Mode** (`Ctrl+Shift+Z`) — animated transitions
- ✅ **Compact Mode** — three density levels, `Ctrl+D` toggle
- ✅ **COMPOSE / NAVIGATE Mode** — vim-like model anchored to composer focus, `j/k` message nav, `r/e/t/p` actions
- ✅ **Status Bar** — mode indicator, density, connection status
- ✅ **Spatial keyboard navigation** between panels
- ✅ **Terminal font reactivity** — Settings changes propagate live to running terminals
- ✅ **About page** — version, build info

---

## 8. Channel Lenses — ✅ Complete

Filtered views per channel: HAS_LINKS, FROM_USER, UNRESOLVED. Inline create/edit/delete. Scope: USER vs CHANNEL. Persistent, shareable.

---

## 9. Incident Mode — ✅ Complete

- ✅ Phase tracking (Detection → Triage → Mitigation → Resolution → Postmortem)
- ✅ Inline phase notes
- ✅ Severity levels (SEV_1 – SEV_4)
- ✅ Action items
- ✅ AI postmortem draft from transcript
- ✅ Backend `listIncidents` query with status/severity filters + cursor pagination

---

## 10. Infrastructure & Quality — ✅ Solid

- ✅ CI/CD: GitHub Actions for both repos (lint, test, coverage, Docker build)
- ✅ Production Dockerfiles for server + UI
- ✅ docker-compose for local dev stack
- ✅ Backend: 353 unit tests + 12 agent_proxy Tauri Rust tests, **95% coverage gate**
- ✅ Frontend: 902 passing tests
- ✅ Security: HMAC-signed CSRF bound to sessions, Stronghold passwords derived from OS keychain, CSP headers, rate limiting, SSRF protection, server membership checks on all workspace state ops
- ✅ WebSocket reconnection: keepalive ping/pong (15s/5s), interruptible backoff via `online` / `visibilitychange`. Worst-case reconnect dropped from ~35s to ~6s.

---

## 11. CLI Client — ❌ Not started

Roadmap-defined as P3 strategic priority. Phase 1 = scriptable `rim` binary (`rim send`, `rim search`, `rim macro run`, `rim watch`). Phase 2 = TUI via `ratatui`. Phase 3 = TUI/CLI convergence.

---

## 12. Killed / Deferred

- ❌ **Embedded Terminal Sessions** — Warp owns this category
- ❌ **Voice / Video (original Phase 6)** — deferred indefinitely; consider Jitsi-as-plugin if demand materializes
- ❌ **E2EE channels** — far future (own dedicated phase)

---

## What's actually broken right now

Honest list of things that look shipped but aren't:

1. **Phase 5 ledger overstates progress.** Ledger marks #1, #2, #7, #9, #11 as completed. Reality: #1, #7, #11 are clean. #2 + #9 had a RED Hawkeye verdict on `db-tests` build that may not have been fully addressed. #4 was marked complete then reverted after Hawkeye returned 6 BLOCKs. #5 is in active remediation.
2. **Agent macro discovery.** `synthesize_command_definitions` doesn't exist. Agent has no path to find macros even though the runtime gate would deny them anyway.
3. **Agent plugin command dispatch.** Frontend tools loader filters on `plugin.enabled` (a TS field) which doesn't exist in the Rust `PluginInfo` shape. So `injectPluginTools` short-circuits and no plugin AI tool is ever registered. Pete's #5 fix addresses this.
4. **Backend plugin_host module is dead code.** 1,290 LOC of contracts that nothing imports. Either move to a shared crate (Phase 6 cleanup) or delete. *(Resolved 2026-10-04: deleted.)*
5. **`db-tests` CI feature flag.** Was fully red across `audit_log_repo`, `message_repo`, `message_attachment_repo`, `message_thread_repo` (mix of new #2 work + pre-existing `forwarded_from_message_id` debt). Strange asked to fold the debt fix into his next pass.
6. **`command.discover` ship gate not started.** This is the actual gate that closes Phase 5. Blocked on the four upstream tasks landing properly.

---

## Suggested framing for next-step decisions

Three categories worth considering:

**Category A — finish what's started.** Drive Phase 5 to an honest close. Strange properly lands #2v2 / #9 / #4 with all six BLOCKs on #4 fixed. Pete lands #5 with the schema fix. Then ship #10 SHIP GATE. Estimate: 3–5 more working days at the current pace, given rework cycles.

**Category B — pivot to the next strategic bet.** ROADMAP names three: Skill Macros + intent routing UX (P2, builds on #4), RIM CLI v1 (P3, no competitors), MCP host/server bridge (P4, RIM is "accidentally MCP-shaped" per Falcon). All three are buildable on what's shipped. CLI v1 is the cheapest unlock and validates the GraphQL contract.

**Category C — clean up before extending.** Address the dead-code in backend `plugin_host`, the schema-drift class of bugs, the `db-tests` feature-flag discipline, and Strange's pre-merge checklist gaps. Phase 6 architecture-cleanup pass. Less glamorous but the team is shipping false positives.

CTO recommendation: A → C → B. Finish Phase 5 honestly, do a 1-day cleanup sprint, then pick the next strategic bet with a clear baseline.

---

*Generated by Tony Stark (CTO) — based on session 2026-04-25 cross-checks of code state, Hawkeye verdicts, and team task ledger. Treat as authoritative ground-truth over `ROADMAP.md` for any short-term planning.*
