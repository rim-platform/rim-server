# Roadmap

_Rebuilt 2026-10-04 after Jakub reset the direction; updated 2026-10-08 with the master plan
that merges the plugin platform and decentralization into five stages. Supersedes the
2026-07-08 roadmap (incident wedge). See `docs/vision.md` for product principles,
`docs/decisions/decentralization-2026-10-08.md` for the key-identity decentralization
decision, `docs/decisions/post-quantum-hybrid-2026-10-09.md` for hybrid post-quantum crypto,
`docs/decisions/relay-proxied-workspaces-2026-10-09.md` for workspace traffic through the
user's relay, and "Where We Are Now" below for the state of both repositories.
The shipped-state ledger is at the end._

## Strategic Thesis — 2026-10-04

RIM is not trying to replace Discord, Slack, or Matrix. It is a **programmable communication
workspace**: a place where chat is the medium and the product is what the user can build on
top of it. Gamers can live in it, developers can live in it, people who want a self-hosted
private place to talk can live in it, and each of them shapes it differently without RIM
shipping a feature for each of them.

Five bets define the product. Everything on this roadmap either serves one of them or is the
minimum support needed so people can use the app daily.

1. **The workspace is programmable by its users.** A command palette over every action,
   keybinds for anything, macros that chain commands, pipelines that react to events, a
   workspace key-value store. This is how the app works, not a workflow builder bolted on.
2. **Plugins extend the application, not just the channel.** A plugin adds panels, commands,
   context actions and keybinds, and runs next to the user with the user's own credentials.
   The canonical example: a panel that shows the running pods in your Kubernetes cluster,
   inside your chat. Slack and Discord bots can only post messages.
3. **An AI agent that operates, not summarizes.** The agent uses the same command registry a
   keybind uses, with capability checks and a human confirmation for anything that mutates.
   With plugins, the agent can operate whatever a community has plugged in.
4. **Keyboard-first, modal, fast.** Vim-style modes, spatial navigation, zen mode, a
   non-Electron desktop. For the people who choose Neovim and Obsidian.
5. **Self-hosted, private and decentralized.** Your box, your model, your audit log today.
   The decentralization model was decided on 2026-10-08: a person is a key pair, history
   lives on their own drive and comes back on any device, servers are interchangeable
   relays, and each workspace is hosted on one server that enforces its roles. No domain or
   home server per user and no central RIM server anyone depends on. End-to-end encryption
   (MLS) covers DMs and private groups; channels stay plaintext so server search, pipelines
   and AI grounding keep working there. Stages 2 to 5 below deliver this.

What this rules out: chasing Discord or Slack feature parity, building first-party verticals
(the incident and on-call layer is gone from the core; it returns as a plugin), and building
voice.

**One sentence:** *RIM is the communication workspace you program: commands, keybinds,
macros, pipelines, plugins and an AI operator on top of chat, where you are a key, your
history lives on your own drive, and no server is central.*

The two pillars that carry the value are the **plugin workspace** (bets 1 to 3) and
**decentralization** (bet 5). Keyboard-first, Vim-style navigation (bet 4) runs through both
as the way the app is used, not as a separate pillar.

## Where We Are Now

_Verified against both repositories on 2026-10-08 (rim-server `a668c05`, rim-ui `b62e31d`)._

**Done since the 2026-10-04 rebuild:** Tracks 0.1 (desktop RCE fixes), 0.3 (repo hygiene:
rim-ui `master` carries the code, `cargo audit` blocking, messaging e2e, resolvers split by
domain, dead `plugin_host` module removed), 0.4 (optional incident module behind a
workspace feature flag), 1.1 (private channels), 1.2 (roles and moderation) and 1.3
(notification rules) are merged on both server and UI `master`. Track 4.1 (general commands
AI-callable) is merged on rim-server (PR #24).

Chat, commands, keybinds, macros and a plugin host all exist, but a plugin command cannot
run yet and the client keeps no history between launches.

| Area | State |
|---|---|
| Chat core | Workspaces, channels (public and private), DMs, group DMs, threads, edit/delete/forward, attachments with quotas, reactions, pins, mentions, read state, inbox, mute, notification rules, presence, typing, link previews, custom emoji, Postgres full-text search, channel lenses, invite-only registration, password reset, SMTP, roles and moderation |
| Programmable runtime | Command registry (~50 builtins, four surfaces, typed args, sensitivity tiers, rate limits), user commands, macros with piping, keybinds with layers, workspace state, pipelines v1, capability registry with default-deny, audit log, confirmation proofs |
| Plugin host (rim-ui) | Separate-process plugins over stdio, capability broker, panels, git install, trust badges. The UI calls `plugin_execute_command`, which does not exist in `src-tauri/src`; `plugin_run_ai_command` always returns `PluginNotRunning` |
| Plugin protocol | Docs and code disagree on method names; only `plugins/hello-world` exists; no SDK |
| Agent | Works (Anthropic and OpenAI, keys in the OS keyring, confirmation modal). Incident, on-call, chat, navigation and workspace-state commands are AI-callable (Track 4.1, PR #24); every agent write goes through the confirmation gate |
| Client history | None kept on disk; TanStack Query memory only |
| Identity | Username and password per server; `Principal` is an in-memory struct with no table |
| Message edits | No `updated_at` or version on `message`, so a client cache cannot see edits yet |
| Shell | Side rail, workspace rail, tool panel, docked thread pane, status bar, spatial navigation, layout presets, zen mode, density modes, modal composer |
| Security | Argon2id, HttpOnly session cookies with HMAC double-submit CSRF, login and reset rate limits, SSRF-safe link previews, storage quotas, CSP on desktop and web |
| Showcase vertical | Dropped 2026-10-10 (tag incidents-last); returns as a plugin after Track 3 |
| rim-cli | Initial commit only |
| Quality | Server CI (fmt, clippy, single-threaded tests, coverage gate, audit, Docker) and client CI (ESLint, Prettier, tsc, Vitest, build, Docker) green |

**Still open from the 2026-10-04 plan:** Track 0.2 embedded terminal decision, audit-log
retention job (waits on variant A or B), coverage gate at 75% against a 95% target.

## Principles for sequencing

- **Enough chat, not parity chat.** Each chat item must be justified as "people cannot live
  here daily without it". The bar is comfort, not Discord.
- **Platform over features.** When a request can be met by a plugin, a bot, a pipeline or a
  macro, build the primitive and the example, not the feature.
- **Do not foreclose Stages 2 to 5.** Every new server-side feature answers two questions:
  what does it look like when message content is encrypted, and what does it look like when
  there is more than one server. See the guardrails section.
- **Prove the platform on ourselves.** Every platform milestone ships with one real thing
  built on it, starting with the Kubernetes pods panel.

## The Plan

Effort: S < 1w, M = 1–3w, L = 1–2mo, XL > 2mo. Tracks keep their numbers so the stages
below can refer to them; the order of work is set by **Stages**, not by track number. Items
marked **Done** are merged on both repositories.

### Track 0 — Foundations and safety (done except 0.2)

1. **Done.** **Close the three desktop RCE findings** (M): terminal creation behind an explicit
   confirmation and a shell allowlist; plugin install URL validation (scheme allowlist, no
   `ext::`, `--` separator, `GIT_ALLOW_PROTOCOL`); no `npm`/`pip` install without a consent
   step that names it as arbitrary code execution.
2. **Open (decision 10).** **Terminal decision** (S): remove the embedded terminal from the
   product (recommended) or keep it as a capability-gated desktop feature. The PTY code is
   also finding S-H1's root.
3. **Done** (audit retention still waits on its variant). **Repo hygiene** (S): merge `rim-ui`
   `feature/initial-code` to `master`; `cargo audit` blocking; close stale issues; schedule
   the audit retention job once the variant is picked; delete the dead server-side
   `plugin_host` module; messaging e2e test that walks pagination both ways; split
   `mutation.rs`/`query.rs`/`types.rs` by domain before more resolvers land.
4. **Done.** **Make the showcase vertical optional** (S/M): a per-workspace feature flag that hides
   the incident and on-call commands, settings and AI surface. Communities that do not run
   services never see SEV levels. This also starts the mechanism Track 2.4 needs for
   workspace-scoped plugin visibility.

### Track 1 — Enough chat (M/L total, bounded)

The daily-comfort list. Items beyond this go through "can a plugin do it?" first.

1. **Done.** **Private channels** with membership, channel browse and join/leave, archive (M). Also
   fix the subscription visibility check that assumes every channel is visible to all
   workspace members.
2. **Done.** **A `Member` role and basic moderation** (M): Owner, Moderator, Member, Guest; kick, ban,
   timeout; delete others' messages; moderation entries in the existing audit log. Per-channel
   overrides and finer permissions only if a real community asks.
3. **Done.** **Notification rules** (M): per-conversation level (all, mentions, nothing), keyword
   alerts, do-not-disturb window, delivered via desktop notifications now and web push once
   Track 1.5 lands.
4. **Message rendering** (S/M): code blocks with language hints, image and video embeds,
   quoting. Markdown remains the format and DOMPurify the backstop; no new content model.
5. **Reach** (L, lower priority): the web build made installable as a PWA with web push and a
   narrow layout per rim-ui `docs/ui-shell.md`. Native mobile out of scope. One-command
   deployment (compose file, backup and restore runbook), which the master plan pulls forward
   to Stage 2 (see Cross-cutting work).

### Track 2 — The plugin platform (bet 2, the main investment, XL total)

1. **Plugin SDK v1** (M): freeze the stdio protocol version, publish a TypeScript SDK and a
   language-agnostic protocol spec, a template repository, and docs that let a stranger build
   a panel plugin in an afternoon. Capability list, manifest schema and panel lifecycle
   documented from the existing implementation, not invented.
2. **The Kubernetes pods panel** (S/M, built with 2.1): the canonical first-party example
   plugin. Reads the user's local kubeconfig, lists pods and their status in a tool panel,
   exposes `k8s.restart_pod` as a Dangerous command so the confirmation gate and the agent
   (Track 4) have something real to operate. Lives in rim-ui (`plugins/k8s-pods/`, with the
   SDK in `sdk/`) until protocol v1 is stable, then moves to its own repository.
3. **Web-capable plugins** (L): a second plugin kind that runs sandboxed in the browser
   (a sandboxed iframe, same capability prompts, same panel slots) for plugins that only need
   the RIM API and the network. Local-process plugins stay desktop-only for things that need
   the machine (kubectl, local files). This is what makes the platform exist on the web
   build and later on mobile.
4. **Workspace-shared plugins** (M/L): server-side plugin registry per workspace (what is
   recommended or required, who approved it, pinned version), signed manifests, install from
   the palette. The marketplace v0 is a curated git-backed index on top of this; economics
   deferred.
5. **Deeper plugin surfaces** (M): plugin commands with sensitivity tiers in the palette and
   keybind picker (partly exists), plugin panels in the side rail and main surface, plugin
   context actions on messages and members, plugin access to workspace state and pipeline
   events, plugin-provided lenses.
6. **Second and third showcases** (S each): a GitHub pull-request panel; a feasibility spike
   on moving the incident and on-call vertical out of the core onto 2.1–2.5. If it cannot be
   moved, the gap is a platform gap and goes on this track.

### Track 3 — Bots, webhooks, pipelines (the lighter on-ramps, L total)

1. **Bot accounts** (M): create a bot in a workspace, issue and revoke tokens, post and read
   as the bot principal, bot badge. Unblocks the seeded system user that escalation and agent
   actions should author as.
2. **Incoming webhooks, generalized** (S/M): the alert receivers become one webhook transport
   with per-channel URLs, message templates, and presets (GitHub, GitLab, CI, Grafana).
3. **Outgoing webhooks and pipeline actions v2** (M): `PostMessage`, `CallWebhook`,
   `SetState`, conditional branches, run history and retries.
4. **Bot-registered commands** (M): a bot registers palette commands backed by its webhook,
   with typed args and sensitivity tiers like builtins.
5. **RIM CLI v1** (M): `rim send`, `rim search`, `rim watch`, `rim macro run`. Doubles as the GraphQL contract test.
6. **MCP bridge** (M): MCP servers install as plugins; RIM exposes itself as an MCP server so
   external agents drive the registry under `OutOfBand` confirmations.

### Track 4 — The agent as operator (bet 3, M/L each)

1. **Done (PR #24).** **Flip the general commands to AI-callable** (S): chat, navigation, state and member
   commands get `nl_description` and `intent_samples`; the agent stops being incident-only.
2. **Agent over plugin commands** (M, with Track 2.2): the agent can call plugin commands
   under the same gate; "restart the crashlooping pod" becomes the demo.
3. **Skill macros and intent routing**: macros as natural-language-routable units with
   per-step enforcement and sensitivity rollup.
4. **Agentic macros** (stepwise execution, pause for confirmation at each mutating step,
   visible transcript). The incident vertical calls these runbooks; everyone else calls them
   automations.
5. **Everyday assistance**: catch-me-up over a channel or thread with citations (FTS exists),
   draft replies, find-and-link. Acting through commands, not a separate chatbot.
6. **Local and self-hosted models**: Ollama and OpenAI-compatible endpoints alongside the
   two providers, so bet 5 holds for the agent too.

### Track 5 — Keyboard-first polish (bet 4, S/M each)

1. Shareable profiles: export and import keybinds, macros and pipelines; a few curated
   starting profiles (minimal, vim, mouse-friendly).
2. Composer and navigation: fuzzy jump-to-anything, command history, per-context keybind
   hints, the open items in rim-ui `docs/ui-shell.md`.
3. Light theme, i18n layer (the two Czech labels are the reminder), accessibility pass.

### Track 6 — Security and sovereignty (bet 5)

1. **Account security — folded into Track 7.2 (Stage 3).** The device list with revoke and
   session rotation come from device keys. TOTP and passkeys are not built for new accounts;
   existing password accounts migrate to a key.
2. **Operator features** (M): OIDC login for teams that want it, data export per user and
   per workspace, audit retention scheduled and configurable, admin audit views.
3. **Privacy posture** (S): metadata minimization pass (what is logged, how long presence is
   kept, what link previews fetch), documented in `docs/security/`.

### Track 7 — Decentralization (XL total, Stages 1 to 5)

Rewritten 2026-10-08. The model is decided (see
`docs/decisions/decentralization-2026-10-08.md`), so the old research spike is dropped and the
track is renumbered 7.0 to 7.4 to match the stages. Local history and keys come first because
both encryption and relays depend on them.

0. **Identity shape in the plugin SDK** (S, Stage 1, with 2.1): plugins see users as
   `{ principalId, displayName }`, never a local UUID or `name@domain`; `principalId` is
   opaque and stable across key changes; mentions are `<@principalId>`; chat methods go
   through one client message service. Ids minted before Stage 3 are provisional (`rimp0_`),
   and the host sends `rim.principal.changed` plus an old-to-new mapping notification when
   Stage 3 replaces them (`docs/specs/plugin-protocol-v1.md`, `docs/specs/key-identity-v1.md`).
1. **Local history and drive archive** (L, Stage 2): an encrypted local message store, local
   search marked "on this device", and an encrypted archive written to a folder, Google Drive
   or Dropbox. Each device writes its own archive segments, so two devices never overwrite one
   file; segments are encrypted under one random archive key that Stage 3 rewraps instead of
   re-encrypting every file. Drive providers are built-in Rust modules behind one trait in the
   Tauri app, not SDK plugins. One server PR adds a message change feed (`change_seq` bumped
   on edit, delete, reaction and pin, plus `conversationChanges(since)`) so edits and deletes
   reach the cache.
2. **Keys, devices and challenge login** (L, Stage 3, absorbs 6.1): inception and device keys
   with a signed rotation log, a signed profile card, QR device linking, a 12-word recovery
   phrase, and challenge login that issues today's session cookie. The identity key stays off
   day-to-day devices, which hold only device keys it signed. Keys are nullable columns on the
   existing `user` row with a kind of local or remote, so none of the dozens of tables that
   reference the user id move. Bots, ingest tokens and the CLI keep host-local credentials and
   host-scoped ids. The web app becomes a linked device of the desktop app.
   _Shipped 2026-10-09_ (`docs/plans/stage-3-remaining-2026-10-09.md` lists the PRs): the
   `rim_identity` crate, enrollment, devices, rotation and recovery, profile cards, challenge
   login, algorithm-agile statements, key-only signup, device linking by code (desktop and
   web), the 12-word recovery phrase, the identity-key backup on the drive, and sign-out of
   other sessions (`docs/specs/key-identity-v1.md`, `docs/specs/device-linking-v1.md`). Web
   plugins and workspace plugin registries with signed manifests (Tracks 2.3 and 2.4) shipped
   with it (`docs/specs/web-plugins-v1.md`, `docs/specs/workspace-plugins-v1.md`).
3. **E2EE v1 with MLS** (XL, Stage 4): OpenMLS on the Tauri side for DMs and private groups;
   channels stay plaintext. Every server feature that reads text gets a client replacement in
   encrypted conversations; plugins read messages only through the client message service under
   their grant. The client sends mentioned principal ids beside the ciphertext so mentions still
   notify, attachments are encrypted on the client, and the server accepts one MLS commit per
   epoch so racing commits cannot fork a group. MLS group state is never backed up: a new
   device joins each group fresh and reads older history from the 7.1 archive. Message
   franking for reports is designed at the start of this stage. Key agreement uses a hybrid
   X25519 + ML-KEM-768 suite.
4. **Relays and workspace hosts** (XL, Stage 5, three steps): rim-server gains relay and
   workspace-host roles and a signed server-to-server HTTP API (HTTP Message Signatures with
   replay protection and rotatable server keys); GraphQL stays the client contract.
   - **7.4a** Cross-server DMs and the request inbox.
   - **7.4b** Remote workspace members: the user's relay subscribes to each workspace host
     with a delegation the user's device signed and forwards traffic both ways, so the client
     keeps one endpoint.
   - **7.4c** The public relay `relay.rim.chat` with its runbook and abuse policy, where
     registering a key needs proof-of-work or an invite.

   The client keeps one GraphQL endpoint and one WebSocket, to its own relay; the relay is an
   untrusted courier for all its users' traffic, workspace traffic included. Every write
   carries the user's device signature and is forwarded unchanged, the host signs its events
   and the client verifies them against the host key pinned from the invite, and the host stays
   the only authority and sequencer. Hosts accept any relay by default, with an optional
   allow-list. A direct connection to a host is the fallback when the relay is unreachable.
   Stage 5 ships after Stage 4, so DMs and private groups are ciphertext on relays; plaintext
   channels are readable by the relay while queued. See
   `docs/decisions/relay-proxied-workspaces-2026-10-09.md`; the contract is
   `docs/specs/server-to-server-v1.md`.

### Architecture guardrails (apply from today)

These keep Tracks 1–6 from making Stages 2 to 5 a rewrite. They cost little now.

- **Message identity is global, not server-local.** Keep UUIDv7 ids, never expose row ids,
  and do not key anything on "the server". Design new tables with an origin/home column in
  mind even if it is a constant today.
- **Content and metadata are separable.** New message features store rendered or derived
  data (search vectors, link previews, mention rows) in a way that can be absent for an
  encrypted message without breaking the row. No new server feature may *require* reading
  message text; it may *benefit* from it.
- **Keep the client capable.** Commands, macros, keybinds, lenses and plugin logic stay
  evaluable on the client. Prefer server features that operate on metadata (membership,
  timing, structure) over content.
- **Identity and devices are first-class.** The seeded system user, bot principals and the
  coming device list all go through one principal model, so keys and remote users (7.2,
  7.4) add a kind and a key to a principal rather than a new concept. Plugins and new APIs see
  an opaque `principalId`, never a local UUID or `name@domain`.
- **GraphQL stays the client contract.** Server-to-server traffic (Stage 5) is a small signed
  HTTP API, a non-GraphQL transport in the `file_handlers.rs` pattern, not federation over
  GraphQL.
- **Migrations are additive.** Expand, then contract a release later, so a host can roll back
  the binary.
- **Single-node stores stay behind traits.** Broadcaster, confirmation store, rate limiters,
  presence and typing are in memory behind interfaces today; keep it that way and never let
  a new feature depend on their single-node semantics.
- **No new server-only vertical features.** The incident layer was removed from the core;
  anything comparable goes through plugins, bots or pipelines.

## Stages (the master plan, 2026-10-08)

Five stages replace the old phase table. They alternate pillars so each one ships something a
user can feel, and stopping after any stage still leaves a better product. The plugin pillar
reaches its showcase in Stage 1 before any decentralization work starts.

| Stage | Tracks | Size | The user gets |
|---|---|---|---|
| 1. Plugins run | 2.1, 2.2, 4.1, 4.2 with 7.0; palette and keybind part of 2.5 | L | Install the k8s pods panel, see pods next to chat, restart one with a keybind or by asking the agent, behind a confirmation |
| 2. Your history is yours | 7.1 | L | Instant start, offline reading, and full history on a fresh install from the drive alone |
| 3. You are a key | 7.2 (absorbs 6.1), 2.3, 2.4 | L | No password for new accounts, a device list with revoke, and plugins that work in the web app |
| 4. Private DMs and groups | 7.3 | XL | A server operator with full database access cannot read their DMs |
| 5. No central server | 7.4a, 7.4b, 7.4c | XL | A DM with someone on another server, a workspace shared across servers, and the freedom to switch relay with no data loss |

**Stage 1 in more detail.** Plugin commands execute from the palette, keybinds and the agent.
Protocol v1 is frozen with the identity shape from 7.0. A TypeScript SDK, a template and a
spec let a stranger build a panel plugin in an afternoon. The Kubernetes pods panel ships
with `k8s.restart_pod` as a Dangerous, AI-callable command, and the general chat, navigation
and state commands become AI-callable too (4.1). The SDK and the k8s plugin live inside
rim-ui (`sdk/`, `plugins/k8s-pods/`) until protocol v1 is stable, then move to their own
repositories.

**Side lane, any time:** Track 1.4 message rendering (S/M), 3.5 RIM CLI v1 (M, host-local
credentials), 5.2 keyboard navigation polish and the Vim profile from 5.1 (S/M), 3.1 to 3.3
bots and webhooks (L), and the audit retention job once its variant is picked. These do not
touch either pillar's critical path.

**Where the pillars depend on each other.** Only one dependency is on the critical path now:
the user id shape must be settled inside protocol v1 before the SDK freezes (7.0). Every
later link runs from decentralization up into plugins, so the plugin pillar never waits on
crypto work: the Stage 2 local index lets `rim.chat.search_messages` answer from the device;
Stage 3 keys sign manifests for workspace-shared plugins and make the web app a linked device;
Stage 4 decrypts for a plugin only through the message service under that plugin's grant;
Stage 5 puts the workspace plugin registry on the workspace host, next to roles and bans.

### First PRs, in order

| # | Repo | PR | Stage | Size |
|---|---|---|---|---|
| 1 | rim-server | The master plan into this roadmap, plus `docs/decisions/decentralization-2026-10-08.md` | all | S |
| 2 | rim-ui | Plugin command execution: `rim.command.execute` through the broker, a real `plugin_execute_command`, `plugin_run_ai_command` on the same path after its consent gates, timeouts and a typed error | 1 | M |
| 3 | rim-ui | Protocol v1 payloads: `{ principalId, displayName }` everywhere, `<@principalId>` mentions, chat methods through one client message service, `rim.shutdown`, conformance tests including id opacity | 1 | M |
| 4 | rim-server | `docs/specs/plugin-protocol-v1.md`; `local_plugin_host.md` and `plugin_model.md` rewritten to match the code | 1 | S |
| 5 | rim-ui | TypeScript SDK in `sdk/`, template plugin, hello-world ported, Python example kept | 1 | S/M |
| 6 | rim-ui | `plugins/k8s-pods/` panel with `k8s.restart_pod` (Dangerous, AI-callable); plugin commands in the palette and keybind picker | 1 | S/M |
| 7 | rim-server | Track 4.1: `nl_description` and `intent_samples` on chat, navigation and state commands (merged, #24) | 1 | S |
| 8 | rim-server | Message change feed: `change_seq` bumped on edit, delete, reaction and pin, plus `conversationChanges(since)` | 2 | S |
| 9 | rim-ui | Encrypted SQLite cache (`rusqlite` with `bundled-sqlcipher`) behind Tauri commands, read-through and reconcile | 2 | M |
| 10 | rim-ui | FTS5 local search as a second palette source | 2 | S |

PRs 7 and 8 have no dependencies and can go in parallel with anything above them. After PR 10
come the archive spec, the archive writer with the local-folder provider and restore, then the
Google Drive and Dropbox providers.

## Cross-cutting work (every stage)

The stages say what ships; this work decides whether other people can install, trust, update
and run it, which a decentralized product depends on more than a hosted one.

| Area | What the plan commits to | Lands by |
|---|---|---|
| Keyboard-first and Vim (Track 5) | A plain default shortcut set that works without modes; Vim mode as an opt-in modal layer (normal, insert, `:` command line opening the palette, `hjkl` across panes); every plugin command bindable in the keybind picker; a shareable Vim profile. Definition of done for every stage: each new surface is reachable without a mouse | Stage 1 (plugin bindings), side lane in Stages 1 to 2 (Vim layer, profiles) |
| Plugin trust and distribution | v1 installs from a git URL with the existing trust badge and capability consent; the consent says plainly that a local-process plugin is not sandboxed and runs as you. Web plugins (2.3) run in a sandboxed iframe. Manifests are signed by a publisher key in Stage 3; a curated git-backed index sits on 2.4. Plaintext of encrypted conversations only through a per-plugin grant | Stages 1, 3, 4 |
| Releases and updates | Tauri updater with signed updates, so a crypto or protocol fix reaches every desktop. A versioned server Docker image per release. Migrations stay forward-only and additive (expand, then contract a release later) so a host can roll back the binary. Plugin protocol, archive format and server-to-server API each support the current and previous version | Before the first public release, at the latest before Stage 3 |
| Self-hosting and operations | One-command compose file with backup and restore (pulled forward from Track 1.5), `/healthz` and Prometheus metrics, relay queue and rate-limit metrics, an upgrade guide per release | Compose by Stage 2, metrics before Stage 5 |
| First run and docs | One first-run path that grows per stage: install a plugin (1), choose a drive (2), create a key and write down the phrase (3), pick a relay (5). Docs: plugin guide (1), self-host guide (2), all specs in `docs/specs/` | Every stage |
| Testing | Plugin conformance suite (1); golden files for the archive format (2); crypto test vectors and an outside review (3, 4); a two-server docker compose test in CI (5); a version-skew test of a new client against the previous server release (from 3) | Per stage |
| Open source and public specs | Others must be able to run rim-server and build clients, so the plugin protocol, archive format, profile card and server-to-server API are published specs. rim-ui and rim-cli need a license before anyone outside can run them (rim-server is Apache-2.0) | Before the first public release |
| Post-quantum (hybrid) | Classical plus post-quantum together, never post-quantum alone: X25519 + ML-KEM-768 (FIPS 203) for MLS key agreement; Ed25519 + ML-DSA-44 (FIPS 204) for identity and device keys, profile cards, login proofs, MLS credentials and server-to-server signatures. Stage 3 formats carry an algorithm tag and room for multi-kilobyte keys and signatures. See `docs/decisions/post-quantum-hybrid-2026-10-09.md` | Formats in Stage 3, key agreement in Stage 4, signatures after Stage 4 |
| Mobile | Out of scope; PWA after Stage 3 (Track 1.5); the cache and MLS code is a Rust crate a future Tauri mobile build can reuse | After Stage 3 |
| rim-cli (Track 3.5) | `rim send`, `search`, `watch`, `macro run` with a host-local token; doubles as the GraphQL contract test | Side lane, any time |

## Track mapping (old numbers to stages)

Tracks 0 to 6 keep their numbers; only Track 7 is rewritten and 6.1 is folded in.

| Track | Status after the master plan | Stage |
|---|---|---|
| 0.1, 0.3, 0.4 | Done | done |
| 0.2 Terminal decision | Open (decision 10) | side lane |
| 1.1, 1.2, 1.3 | Done | done |
| 1.4 Message rendering | Unchanged | side lane |
| 1.5 Reach (PWA, deploy) | Unchanged; web keys depend on Stage 3 | after 3 |
| 2.1 Plugin SDK v1 | Includes 7.0 identity rules | 1 |
| 2.2 Kubernetes pods panel | In rim-ui first | 1 |
| 2.3 Web-capable plugins | Moved earlier, with the web app as a linked device | 3 |
| 2.4 Workspace-shared plugins | Signed manifests; registry lives on the workspace host | 3 |
| 2.5, 2.6 Deeper surfaces, more showcases | Palette and keybind part pulled into Stage 1 | 1, then later |
| 3.1 to 3.4 Bots, webhooks, pipelines | Unchanged; bots keep host-local credentials | side lane |
| 3.5 RIM CLI v1 | Unchanged; host-local credentials | side lane |
| 3.6 MCP bridge | Unchanged | later |
| 4.1 General commands AI-callable | Done (PR #24) | 1 |
| 4.2 Agent over plugin commands | Pulled into Stage 1 | 1 |
| 4.3 to 4.6 | Unchanged | later |
| 5.x Keyboard polish | Vim profile and navigation as side lane | side lane |
| 6.1 Account security | Folded into 7.2 | 3 |
| 6.2, 6.3 Operator, privacy | Unchanged; workspace export reused by 7.4 | later |
| 7.0 Identity shape in SDK (new) | Was not in the roadmap | 1 |
| 7.1 Local history and drive archive (new) | Replaces the old design spike | 2 |
| 7.2 Keys, devices, challenge login | Was "device and key infrastructure" | 3 |
| 7.3 E2EE v1 with MLS | DMs and private groups | 4 |
| 7.4 Relays and workspace hosts (4a, 4b, 4c) | Was "decentralization, decide later" | 5 |

## Risks

- **Scope.** Two XL pillars on one small team; Stages 4 and 5 are each XL. Every stage ships
  alone, and the plugin pillar reaches its showcase first.
- **Freezing the SDK too early or too late.** v1 carries only what is implemented plus what a
  panel plugin needs; pre-Stage 3 ids are provisional and migrate once via
  `rim.principal.changed` plus a mapping notification.
- **Plugins and encrypted messages.** A local plugin runs with the user's rights; decryption
  for plugins only through the client message service, per plugin and per grant, shown in the
  consent prompt.
- **Key loss.** Recovery phrase and backup are mandatory at first run with a banner until
  done; the recovery key in the rotation log restores identity even without the drive.
- **Crypto correctness.** Stages 3 and 4 need an outside review before release, and OpenMLS
  rather than hand-rolled primitives.
- **Keys in the web app.** A browser has no OS keyring; non-extractable WebCrypto keys in
  IndexedDB are weaker than desktop, and the UI should say so.
- **Spam without domain reputation.** Request inbox and rate limits per IP, key and server
  first; proof-of-work on first contact is reserved in the `send` API from v1.
- **Server features that read text.** Search, keyword notifications, lenses, link previews and
  pipelines need client replacements in encrypted conversations; Stage 2's local index is the
  first, so Stage 4 does not start without it.

## Anti-Roadmap — explicitly not building

- **Feature parity with Discord or Slack.** Not the goal; Track 1 is bounded on purpose.
- **Voice and video.** Discord's job; if a community needs calls, a bridge plugin to a
  self-hosted media server is the path, and it is not scheduled.
- **First-party verticals.** No gaming pack, no developer pack, no second incident-style
  core module. Showcase plugins instead.
- **A native media stack, integration-directory breadth, enterprise agent platform.**
- **Marketplace monetization** before there is a network of plugins.
- **Domain federation (`name@domain` as identity), Matrix-style room replication, or
  peer-to-peer.** Rejected for the key-identity model; a domain may return later only as an
  optional alias on top of a key. See `docs/decisions/decentralization-2026-10-08.md`.
- **Encrypted channels.** Channels stay plaintext so server search, pipelines and the agent
  keep working there; E2EE covers DMs and private groups.

## Open Decisions (for Jakub)

From the 2026-10-08 master plan. Work proceeds on the recommended answer until told otherwise.
Decision 1 (go on the first PRs) was answered "go" on 2026-10-08.

| # | Question | Options | Recommended |
|---|---|---|---|
| 2 | Stage order: plugins first, or both pillars in parallel from day one? | plugins first / parallel | plugins first |
| 3 | Where do the SDK and the k8s plugin live? | rim-ui / new repos | rim-ui for now |
| 4 | Publish the SDK to npm in v1? | yes / no | no, install from git |
| 5 | Fold Track 6.1 (TOTP, passkeys) into key login? | fold / keep both | fold |
| 6 | May plugins read decrypted DMs, with a per-plugin grant? | grant / never | grant |
| 7 | First drive providers after the local folder? | Google Drive / Dropbox / both | Google Drive |
| 8 | Is the first audience developers and ops? | yes / broader | yes |
| 9 | Readable names for keys in Stage 5? | contact lists / alias / directory | contact lists |
| 10 | Embedded terminal (still in `src-tauri/src/terminal`)? | remove / keep gated | remove |
| 11 | Audit-log retention variant? | A (90d/365d) / B (365/365) | A |
| 12 | Incidents and on-call: spike moving them to a plugin, or commit now? | spike / commit | dropped |
| 13 | License for rim-ui and rim-cli (rim-server is Apache-2.0)? | Apache-2.0 / AGPL / MIT | Apache-2.0 |
| 14 | Vim mode: opt-in, or on by default? | opt-in / default | opt-in |
| 15 | Mobile: PWA after Stage 3, or native later? | PWA / native | PWA |

Still open from the decentralization design, none blocking before Stage 5: whether relays
delete messages once the drive has them, Matrix interop, and whether losing phrase, drive and
devices together is an acceptable total loss (the plan assumes yes). A host accepts any relay
by default, with an optional allow-list (decided 2026-10-09). The 2026-10-04 open decisions not listed above are
settled: the master plan adopts sandboxed iframes for web plugins and DMs plus private groups
for E2EE v1, Jakub confirmed the decentralization model (see the decision record), and the
four-role ladder shipped in 1.2.

Carried: `oncall.take_shift` authorization, `.env` history purge, formal ratification of the
client-side plugin reframe.

## Ledger — what the previous roadmaps shipped

Phases 0–4 of the April 2026 roadmap (communication core, command system, keybinds,
plugins, incident mode v1, AI agent reframe phases 1–5) and Tracks A, B, C1, C2 of the July
2026 roadmap (foundations, search/alerts/email, on-call and escalation, agent-operator
fusion) are merged on `rim-server` `master` (PR #11, 2026-10-04) and on `rim-ui` `master`
(through `feature/initial-code`, merged in rim-ui PR #1). Point-in-time specs live in
`docs/specs/`, `docs/design/`, `docs/plans/` (rim-ui) and `docs/research/`.

Tracks 0.1, 0.3, 0.4 and 1.1 to 1.3 of the 2026-10-04 roadmap are merged on both `master`
branches (rim-server through PR #21, `a668c05`; rim-ui through PR #9, `b62e31d`).
