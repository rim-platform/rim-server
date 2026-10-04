# Roadmap

_Rebuilt 2026-10-04 after Jakub reset the direction. Supersedes the 2026-07-08 roadmap
(incident wedge). See `docs/vision.md` for product principles and
`docs/next-steps-2026-10-04.md` for the verified state of both repositories. The shipped-state
ledger is at the end._

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
5. **Self-hosted, auditable, and converging on private.** Your box, your model, your audit
   log today. **End-to-end encryption is a committed future version**, and the platform
   **converges toward a decentralized chat platform**. Both are horizons with real tension
   against bets 1–3 (server-side search, pipelines and AI grounding need plaintext on the
   server), so the roadmap treats them as architecture constraints now and as feature work
   later, rather than promising them in the next release.

What this rules out: chasing Discord or Slack feature parity, building first-party verticals
(the incident and on-call layer stays as the first showcase of what the platform can host,
it is no longer a pillar), and building voice.

**One sentence:** *RIM is the self-hosted communication workspace you program: commands,
keybinds, macros, pipelines, plugins and an AI operator on top of chat, built to become
private and decentralized.*

## Where We Are Now

_Verified against code and GitHub on 2026-10-04, both repositories._

**Shipped and solid:**
- **Chat core**: workspaces, channels, DMs, group DMs, threads, edit/delete/forward,
  attachments with quotas, reactions, pins, mentions, read state, inbox, mute, presence,
  typing, link previews, custom emoji, Postgres full-text search, channel lenses, invite-only
  registration with operator bootstrap, password reset, SMTP.
- **Programmable runtime (bet 1)**: command registry (~50 builtins, four surfaces, typed
  args, sensitivity tiers, rate limits), user commands, macros with piping, 18 default
  keybinds with layers, workspace state (KV, TTL, atomic increment, events), pipelines v1
  (six triggers, one action), capability registry with default-deny, audit log, confirmation
  proofs, skill-tier progressive disclosure, `command.discover`.
- **Plugins (bet 2)**: desktop-only local plugin host in Rust (20 modules): separate-process
  plugins over stdio, manifest validation, capability broker with real enforcement, panels
  over a locked `rim-plugin://` scheme, git install, trust badges, plugin AI consent. No
  public SDK or docs for third parties yet; nothing runs on the web build.
- **Agent (bet 3)**: client-side, Anthropic and OpenAI, keys in the OS keyring via the Rust
  proxy, tool calls through the unified dispatcher, confirmation modal, audit emission. Only
  the incident and on-call commands are flipped to AI-callable so far.
- **Shell (bet 4)**: side rail, workspace rail, tool panel, docked thread pane, status bar,
  spatial navigation, layout presets, zen mode, density modes, modal composer, 15 settings
  tabs.
- **Security (bet 5)**: Argon2id, HttpOnly session cookies with HMAC double-submit CSRF,
  login and reset rate limits, SSRF-safe link previews, storage quotas, membership-gated
  access everywhere, CSP on desktop and web, no tokens in local storage.
- **First showcase vertical**: incidents, on-call schedules, escalation policies with timers,
  alert webhooks (generic, Alertmanager, Grafana), agent-driven incident commands.
- **Quality**: server CI (fmt, clippy, single-threaded tests, 75% coverage gate, audit,
  Docker); client CI (ESLint, Prettier, tsc, 1,348 tests, build, Docker). Green on fresh
  clones of both.

**Known debt, still open:**
- Client: three RCE-class desktop findings (`rim-ui/fable-findings.md` S-H1..S-H3): unguarded
  terminal spawn, `git clone` with unvalidated URL, package-manager install before consent.
  The embedded terminal killed in April is still shipped. Reconnect refetch storm, hardcoded
  endpoint, two error boundaries, `MessageItem.tsx` god file, untested pipelines/lenses.
- Server: no through-the-stack messaging e2e (paginator rework unverified), resolver god
  files, audit retention never scheduled, dead `rim_services/plugin_host` module,
  `cargo audit` advisory-only.
- Both: `rim-ui` `master` is the initial commit; all code is on `feature/initial-code`.

## Principles for sequencing

- **Enough chat, not parity chat.** Each chat item must be justified as "people cannot live
  here daily without it". The bar is comfort, not Discord.
- **Platform over features.** When a request can be met by a plugin, a bot, a pipeline or a
  macro, build the primitive and the example, not the feature.
- **Do not foreclose the horizons.** Every new server-side feature answers two questions:
  what does it look like when message content is encrypted, and what does it look like when
  there is more than one server. See the guardrails section.
- **Prove the platform on ourselves.** Every platform milestone ships with one real thing
  built on it, starting with the Kubernetes pods panel.

## The Plan

Effort: S < 1w, M = 1–3w, L = 1–2mo, XL > 2mo. Track 0 first; Tracks 1 and 2 in parallel;
Track 2 gets the majority of effort for the next two quarters.

### Track 0 — Foundations and safety (first, M total)

1. **Close the three desktop RCE findings** (M): terminal creation behind an explicit
   confirmation and a shell allowlist; plugin install URL validation (scheme allowlist, no
   `ext::`, `--` separator, `GIT_ALLOW_PROTOCOL`); no `npm`/`pip` install without a consent
   step that names it as arbitrary code execution.
2. **Terminal decision** (S): remove the embedded terminal from the product (recommended) or
   keep it as a capability-gated desktop feature. The PTY code is also finding S-H1's root.
3. **Repo hygiene** (S): merge `rim-ui` `feature/initial-code` to `master`; `cargo audit`
   blocking; close stale issues; schedule the audit retention job once the variant is picked;
   delete the dead server-side `plugin_host` module; messaging e2e test that walks
   pagination both ways; split `mutation.rs`/`query.rs`/`types.rs` by domain before more
   resolvers land.
4. **Make the showcase vertical optional** (S/M): a per-workspace feature flag that hides
   the incident and on-call commands, settings and AI surface. Communities that do not run
   services never see SEV levels. This also starts the mechanism Track 2.4 needs for
   workspace-scoped plugin visibility.

### Track 1 — Enough chat (M/L total, bounded)

The daily-comfort list. Items beyond this go through "can a plugin do it?" first.

1. **Private channels** with membership, channel browse and join/leave, archive (M). Also
   fix the subscription visibility check that assumes every channel is visible to all
   workspace members.
2. **A `Member` role and basic moderation** (M): Owner, Moderator, Member, Guest; kick, ban,
   timeout; delete others' messages; moderation entries in the existing audit log. Per-channel
   overrides and finer permissions only if a real community asks.
3. **Notification rules** (M): per-conversation level (all, mentions, nothing), keyword
   alerts, do-not-disturb window, delivered via desktop notifications now and web push once
   Track 5.1 lands.
4. **Message rendering** (S/M): code blocks with language hints, image and video embeds,
   quoting. Markdown remains the format and DOMPurify the backstop; no new content model.
5. **Reach** (L, lower priority): the web build made installable as a PWA with web push and a
   narrow layout per `docs/ui-shell.md`. Native mobile out of scope. One-command deployment
   (compose file, backup and restore runbook).

### Track 2 — The plugin platform (bet 2, the main investment, XL total)

1. **Plugin SDK v1** (M): freeze the stdio protocol version, publish a TypeScript SDK and a
   language-agnostic protocol spec, a template repository, and docs that let a stranger build
   a panel plugin in an afternoon. Capability list, manifest schema and panel lifecycle
   documented from the existing implementation, not invented.
2. **The Kubernetes pods panel** (S/M, built with 2.1): the canonical first-party example
   plugin. Reads the user's local kubeconfig, lists pods and their status in a tool panel,
   exposes `k8s.restart_pod` as a Dangerous command so the confirmation gate and the agent
   (Track 4) have something real to operate. Lives in its own repository as the SDK's proof.
3. **Web-capable plugins** (L): a second plugin kind that runs sandboxed in the browser
   (iframe or worker, same capability prompts, same panel slots) for plugins that only need
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
5. **RIM CLI v1** (M): `rim send`, `rim search`, `rim watch`, `rim macro run`. Design in
   `docs/design/cli-v1-design-2026-04-22.md`. Doubles as the GraphQL contract test.
6. **MCP bridge** (M): MCP servers install as plugins; RIM exposes itself as an MCP server so
   external agents drive the registry under `OutOfBand` confirmations.

### Track 4 — The agent as operator (bet 3, M/L each)

1. **Flip the general commands to AI-callable** (S): chat, navigation, state and member
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
   hints, the open items in `docs/ui-shell.md`.
3. Light theme, i18n layer (the two Czech labels are the reminder), accessibility pass.

### Track 6 — Security and sovereignty (bet 5, now)

1. **Account security** (M): TOTP and passkeys, session rotation and idle expiry, device
   list with revoke, login alerts.
2. **Operator features** (M): OIDC login for teams that want it, data export per user and
   per workspace, audit retention scheduled and configurable, admin audit views.
3. **Privacy posture** (S): metadata minimization pass (what is logged, how long presence is
   kept, what link previews fetch), documented in `docs/security/`.

### Track 7 — The horizons: end-to-end encryption and decentralization (XL, later)

Committed direction, not next-release work. The order is deliberate: encryption first, then
decentralization, because an encrypted single-server product is useful on its own and most
of its client-side machinery (keys, devices, verification) is reused by decentralization.

1. **Research and design spike** (M, start during Track 2): pick the primitives (MLS is the
   obvious candidate for group encryption; Matrix's Olm/Megolm the alternative), decide what
   is encrypted first (DMs and private groups), and write down honestly what stops working in
   encrypted conversations: server-side search, pipelines reading content, link previews, AI
   grounding, message-content lenses. Define the client-side replacements (local search index,
   client-side pipeline evaluation, plugin-side AI) and which bets 1–3 features move to the
   client as a result.
2. **Device and key infrastructure** (L): per-device identity keys, device verification,
   key backup with a user secret, all stored via the existing keyring path. Shipped before any
   message is encrypted so the UX is proven.
3. **E2EE v1** (L): DMs and private groups encrypted end to end; channels stay plaintext
   with server features; the UI makes the difference visible. Local search index for
   encrypted conversations. Agent and pipelines operate on encrypted content only on the
   client, under the same capability gates.
4. **Decentralization** (XL, after E2EE v1): decide the model in a research spike during
   Track 7.1: federation between RIM servers (users on different homes share a workspace),
   Matrix interoperability (bridge or native), or peer-to-peer. The recommendation today is
   server-to-server federation with portable identities, because it preserves the plugin and
   command model and reuses the E2EE device layer; Matrix interop as a bridge plugin, not a
   rewrite. Decide with evidence, not now.

### Architecture guardrails (apply from today)

These keep Tracks 1–6 from making Track 7 a rewrite. They cost little now.

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
  coming device list all go through one principal model, so federation adds a home to a
  principal rather than a new concept.
- **Single-node stores stay behind traits.** Broadcaster, confirmation store, rate limiters,
  presence and typing are in memory behind interfaces today; keep it that way and never let
  a new feature depend on their single-node semantics.
- **No new server-only vertical features.** The incident layer is grandfathered; anything
  comparable goes through plugins, bots or pipelines.

## Anti-Roadmap — explicitly not building

- **Feature parity with Discord or Slack.** Not the goal; Track 1 is bounded on purpose.
- **Voice and video.** Discord's job; if a community needs calls, a bridge plugin to a
  self-hosted media server is the path, and it is not scheduled.
- **First-party verticals.** No gaming pack, no developer pack, no second incident-style
  core module. Showcase plugins instead.
- **A native media stack, integration-directory breadth, enterprise agent platform.**
- **Marketplace monetization** before there is a network of plugins.
- **End-to-end encryption or decentralization promised for the next release.** They are
  committed horizons with guardrails now; shipping dates come from the Track 7 spikes.

## Sequencing (proposed)

| Phase | Weeks | Content |
|---|---|---|
| 1 | 1–5 | Track 0 complete; Track 1.1 private channels; Track 2.1 SDK docs started |
| 2 | 6–14 | Track 2.1 SDK v1 and 2.2 Kubernetes panel shipped; Track 1.2 roles and moderation, 1.3 notifications; Track 4.1 general commands AI-callable; Track 6.1 account security |
| 3 | 15–26 | Track 2.3 web plugins and 2.4 shared plugins; Track 3.1–3.3 bots, webhooks, pipeline actions; Track 4.2 agent over plugins; Track 7.1 E2EE design spike; Track 1.4 rendering |
| 4 | 27+ | Track 2.5–2.6; Track 3.4–3.6 CLI and MCP; Track 4.3–4.6; Track 5; Track 6.2–6.3; Track 1.5 PWA; Track 7.2 device and key infrastructure |

E2EE v1 and the decentralization decision follow the Track 7 spikes and are not dated here.

## Open Decisions (for Jakub)

1. **Embedded terminal**: remove (recommended) or keep capability-gated.
2. **Incidents and on-call**: keep in core as an optional module for now, with a Track 2.6
   spike on moving them to a plugin (recommended), or commit to the move up front.
3. **Web plugin runtime**: sandboxed iframe plugins (recommended, reuses the panel model) or
   worker-based with a RIM-provided UI toolkit.
4. **E2EE v1 scope**: DMs and private groups first (recommended), or all conversations
   including channels with server-side features disabled.
5. **Decentralization model**: decide after the Track 7.1 spike; today's lean is server
   federation with portable identity, Matrix as a bridge.
6. **Roles depth**: four-role ladder (recommended) or arbitrary roles with permission
   bitfields from the start.
7. **Mobile**: PWA-first (recommended) or native.

Carried: audit retention variant (A 90d/365d vs B 365/365), `oncall.take_shift`
authorization, `.env` history purge, formal ratification of the client-side plugin reframe.

## Ledger — what the previous roadmaps shipped

Phases 0–4 of the April 2026 roadmap (communication core, command system, keybinds,
plugins, incident mode v1, AI agent reframe phases 1–5) and Tracks A, B, C1, C2 of the July
2026 roadmap (foundations, search/alerts/email, on-call and escalation, agent-operator
fusion) are merged on `rim-server` `master` (PR #11, 2026-10-04) and on `rim-ui`
`feature/initial-code` (`d907c86`). Point-in-time specs live in `docs/specs/`,
`docs/design/`, `docs/plans/` (rim-ui) and `docs/research/`.
