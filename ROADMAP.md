# Roadmap

_Rebuilt 2026-07-08 (Fable review + Doctor Strange market recon). Supersedes the
2026-04-25 roadmap; its phase history and shipped-state ledger are preserved below.
See `docs/vision.md` for product vision, `fable-notes.md` (both repos) for the
architecture review this roadmap incorporates._

## Strategic Thesis — updated 2026-07-08

RIM is a programmable communication workspace. That thesis is unchanged and the market
has moved *toward* it: Slack turned Slackbot into an AI teammate, Mattermost shipped
Agents V2 (sovereign self-hosted AI with grounded citations, June 2026), Rocket.Chat
runs local LLMs, and every incident-tooling vendor (incident.io, Rootly, Grafana IRM)
ships "AI SRE" that *suggests* actions inside chat it doesn't own. FireHydrant was
absorbed into Freshworks (Jan 2026) — the incident category is consolidating into
expensive SaaS exactly as self-hosted teams' compliance needs rise.

Two consequences:

1. **"Sovereign / BYO-model AI" is no longer a differentiator — it's table stakes** in
   the self-hosted segment. Mattermost and Rocket.Chat already sell it. Keep it, market
   it, but don't bet the roadmap on it.
2. **What nobody has is the combination RIM already owns in code:** a chat substrate +
   programmable commands/pipelines/macros + a *native* incident layer + a
   capability-gated AI agent with dangerous-command confirmation that can *operate* the
   workspace, not just summarize it. incident.io can't copy this (they rent Slack's
   substrate); Mattermost is closest (Playbooks + Agents V2) but has no programmable
   command/pipeline fusion and no confirmation-gated operator.

**Headline bet: RIM is "Slack + incident.io in one self-hosted box, with an AI agent
that can actually run the incident."** The incident wedge is the go-to-market story;
the programmable runtime (commands, macros, pipelines, plugins, agent) is the moat
underneath it. Target buyer: self-hosting dev/SRE/platform teams.

**The wedge is gated by three missing table stakes:** alert ingestion, real full-text
search, and on-call reachability (mobile/push). Fix the gates, then deepen the wedge.

Prior strategic priorities (Falcon recon 2026-04-22) are folded in, not discarded:
- **P1 AI Agent Reframe** — agent speaks the command registry. Still the spine; Track D
  is its continuation.
- **P2 Skill Macros + intent routing** — becomes the runbook primitive in Track D.
- **P3 RIM CLI v1** — moved to Track E; still zero-competitor whitespace, no longer
  ahead of the wedge.
- **P4 MCP host/server bridge** — Track E; `OutOfBand` ConfirmationProof remains the
  forward-compat scaffolding.

## Where We Are Now

_Last verified against git history 2026-07-08. Both repos idle since 2026-04-29;
the ledger below reflects last known state — **first action item is to re-verify the
Reframe Phase 5 task statuses before resuming.**_

**Shipped and solid** (Phases 0–4 of the original roadmap, condensed):
- **Communication core**: servers/channels/DMs/groups, threads, attachments, reactions,
  pins, mentions, read state, presence, typing, link previews, custom emoji, invites +
  operator-bootstrap registration (redesigned 2026-04-28), password reset.
- **Programmable runtime**: command registry + palette, typed args, user commands,
  chained macros with piping (pre-validation, per-stage results), 19 default keybinds
  (context-aware, customizable), workspace state machine (KV + TTL + events), reactive
  pipelines (v1), channel lenses.
- **Plugin system**: manifest, separate-process local host, capability prompts, panels,
  context actions, sandboxed rendering over `rim-plugin://` with locked CSP.
- **Incident Mode v1**: phases (Detection→…→Postmortem), severity SEV1–4, phase notes,
  action items, AI postmortem draft from transcript, `listIncidents` history query.
- **AI agent**: BYO-key (OS keyring, Rust-side proxy, `Zeroizing`), tool calling routed
  through the unified command dispatcher (Reframe Phase 4 shipped 2026-04-24),
  sensitivity tiers + confirmation proofs (TOCTOU-hardened), capability default-deny.
- **Power-user UX**: Zen mode, compact/density modes, COMPOSE/NAVIGATE vim-style modes,
  `j/k` message actions, status bar, spatial navigation, layout presets, emoji picker.
- **Quality**: CI (fmt, clippy -D warnings, 95% coverage gate, cargo audit, Docker),
  ~350+ backend tests, 900+ frontend tests, WS reconnect hardening.

**In flight / unverified** — Reframe Phase 5 (audit DB, typed_verb, Skill Macros AI
exposure, plugin AI consent, keyring Option B, per-actor rate limits,
`command.discover` ship gate). Last known: tasks #1/#7/#11 at Hawkeye gate, rest
blocked behind them. The 2026-04-27→29 commits ("macros + refactoring", "security
hardening + rate limits") likely landed part of this. **Re-ledger before anything else.**

**Known debt** (Fable review 2026-07-08, see `fable-notes.md` in each repo):
- `mocks` feature compiled into production deps (rim_api/rim_server/rim_infra_services
  Cargo.toml) — trivial fix, do immediately.
- Command↔macro two-phase construction leaves two live command-service instances
  (`provider.rs`) — fragile invariant, fix before more code grows around it.
- Schema/repo wiring duplicated (`build_schema` vs `export_sdl`; `app.rs` vs
  `AppRepos::with_handle`).
- God files: `types.rs` 2.8k, `mutation.rs` 2.4k lines — split via `MergedObject`.
- UI: blanket `invalidateQueries()` on WS reconnect (refetch storm); hardcoded endpoint
  in ≥5 files; only 2 error boundaries; `MessageItem.tsx` 938 lines; no ESLint gate;
  `pipelines/`/`lenses/`/`disclosure/` untested.
- Carried from Phase 5 parking lot: shared `rim_plugin_validation` crate, plugin_host
  clippy errors (#64), multi-node confirmation store, embedding-based discover ranking.

## Competitive Snapshot — mid-2026

| Threat | What they have | RIM's answer |
|---|---|---|
| **Mattermost** (closest) | Agents V2: sovereign AI, grounded+cited answers; Playbooks (checklist incident response); self-hosted | Fuse incidents with programmable pipelines + operator agent; verify Playbooks overlap before Track C commits |
| Slack + incident.io/Rootly | Best-in-class incident UX, AI SRE suggestions — all SaaS, no self-host, rising TCO | Own the substrate: incident-native chat, one box, your model |
| Rocket.Chat | On-prem AI (local Llama), clearance-aware search | Match grounded recall (Track B/D); beat on programmability |
| Element/Matrix | E2EE + federation, government traction | Don't compete there (see anti-roadmap) |
| Zulip | Topic threading, E2EE mobile push (12.0) | Threading parity not required; note their mobile bar |
| Campfire/ONCE | Own-the-code simplicity, one-time price | Pricing/packaging lesson, not a feature threat |

## The Plan

Five tracks, roughly sequential with overlap. Effort: S < 1w, M = 1–3w, L = 1–2mo, XL > 2mo.

### Track A — Foundations (unblocks everything) — S/M total

1. Re-verify and land the **Reframe Phase 5 ship gate** (`command.discover` green) —
   the ledger above is 10 weeks stale.
2. **`mocks` → dev-dependencies** (S, same day).
3. **Fix WS-reconnect refetch storm** — scope invalidation to active-workspace keys
   (S); "resume since cursor" on `workspaceEvents` follows in Track B.
4. **Centralize UI endpoint config** behind `getGraphqlEndpoint()` (S).
5. **Break the command↔macro init cycle** — single command-service instance (M).
6. **Plugin-host install hardening** (fable-findings RCE-class items) — prerequisite
   for any third-party plugin story (M).
7. ESLint + Prettier gate in rim-ui CI; per-surface error boundaries (S).

### Track B — Table-Stakes Gates (adoption blockers for the wedge) — M/L total

1. **Full-text search** (M): Postgres FTS (tsvector + GIN) replacing LIKE; search
   syntax (`from:`, `in:`, `has:link`, date ranges) reusing the lens filter model.
   Prerequisite for grounded AI recall.
2. **Alert ingestion** (M): webhook receivers — generic JSON + Alertmanager + Grafana
   formats — as a new non-GraphQL transport (FL2 pattern). Alerts create/update
   incidents via pipelines; dedup + severity mapping rules.
3. **Email delivery** (S/M): finish the email sender stub; notification digest +
   incident-page fallback.
4. **Slack import** (M): workspace/channel/user/message import from Slack export zips.
   Adoption door-opener; do after 1–3.
5. **Subscription resume-since-cursor** (M): replay missed `workspaceEvents` on
   reconnect instead of cache nuke. Pairs with A3.

### Track C — The Incident Wedge (the product story) — L

_RATIFIED by Jakub 2026-07-10, re-scoped per the Mattermost Playbooks teardown
(docs/research/mattermost-playbooks-teardown-2026-07-10.md). Mattermost owns
checklists and already has grounded cited recall; it has NO native on-call/escalation
(delegates to PagerDuty/Opsgenie) and its agents can't drive runbooks. Weighting
below reflects that._

1. **SPEARHEAD — On-call schedules + escalation policies** as first-class objects
   (rotations, overrides, escalation timers → mention/email/push chain). The
   strongest structural gap vs Mattermost: "incident.io in the box."
2. **Incident lifecycle in the palette** — declare/ack/escalate/resolve as commands,
   composable into macros/pipelines. Build tightly coupled with Track D's operator:
   the commands+agent fusion is the moat, not lifecycle commands alone.
3. **AI postmortem v2** (parity-plus, not the story): draft from the tagged timeline
   with citations; action items → trackable objects.
4. **Grounded recall** (explicit catch-up to Agents V2 parity — defense, not wedge):
   cited answers over the workspace via the BYO model. Needs B1 (shipped).
5. **Incident metrics** (parity): MTTA/MTTR, incident history/archive view (backend
   query already shipped).

### Track D — The Agent as Operator (the moat) — M/L

The differentiated version of "AI SRE": everyone else *suggests*, RIM *acts* under a
human gate.

1. **Skill Macros + intent routing** (old P2): macros as NL-routable units; agent
   dispatches by intent with per-step `ai_callable` enforcement + sensitivity rollup.
2. **Agentic runbooks**: runbook = macro/pipeline the agent walks step-by-step; each
   Mutating/Dangerous step goes through the confirmation flow; execution transcript
   lands in the incident timeline.
3. **Runbook plugins**: teams package runbooks + panels as plugins (needs A6 hardening
   first).
4. **On-call reachability** (L, scope decision needed): thin PWA on the existing web
   build — push notification, ack/escalate, read incident channel. Full native mobile
   stays out of scope; vision/iron-man to validate PWA push viability per platform.

### Track E — Ecosystem (post-wedge) — M/L each

1. **RIM CLI v1** (old P3): `rim send/search/watch/macro run` — scriptable, CI-friendly;
   doubles as a live GraphQL contract validator. TUI later.
2. **MCP host/server bridge** (old P4): MCP servers install as plugins; Claude
   Desktop/Cursor can drive RIM via the same command registry + OutOfBand confirmations.
3. **Plugin marketplace v0** (old P6): git-backed curated manifest, signed;
   `rim plugin install github:user/plugin`. Economics deferred until there's a network.
4. **Reactive Pipelines GA** (old P5): branching, JSON transforms, error handling —
   the "Slack Workflow Builder done right" story.

## Anti-Roadmap — explicitly not building

- **Native voice/video/screen share** — commodity; integrate a Jitsi/Element Call
  bridge for incident calls via plugin if demand materializes. (Carried: deferred
  indefinitely 2026-04-22.)
- **Embedded terminal sessions** — KILLED 2026-04-22 (Warp owns it). Stands.
- **Federation** — Matrix owns it; not what dev/SRE self-hosters ask for.
- **Marketplace monetization** — chicken-and-egg; build primitives, defer economics.
- **Integration-directory breadth / enterprise agent platform** — losing battle vs
  Salesforce/Microsoft by definition; out of thesis.
- **E2EE as a headline** — it directly conflicts with server-side search, AI grounding,
  and incident indexing (the wedge). If/when shipped, scope to DMs; do not promise it
  for the incident/AI surface. One story: *sovereign, searchable, AI-operable*.

## Open Decisions (for Kuba)

Carried from Phase 5 (still unratified as of last ledger):
1. Plugin reframe to client-side architecture (#5) — CTO call was yes; ratify.
2. Audit retention: Variant A (90d/365d) vs B (365/365).
3. Tauri audit event destination: GraphQL→server (recommended) vs local-log only.
4. Audit rows for `command.discover` Safe-tier reads.

New with this roadmap:
5. ~~Ratify the incident-wedge bet~~ — RATIFIED 2026-07-10 (re-scoped: C1 spearhead,
   C2 coupled to Track D, C3/C5 parity, C4 catch-up).
6. Mobile scope: PWA-first (recommended) vs native — gates Track D4.
7. ~~Mattermost Playbooks/Agents V2 teardown~~ — DONE 2026-07-10, see
   docs/research/mattermost-playbooks-teardown-2026-07-10.md.
