# Roadmap

_Rebuilt 2026-10-04 for the general-purpose direction. Supersedes the 2026-07-08 roadmap
(incident-wedge bet). The shipped-state ledger from that roadmap is preserved below and was
re-verified against both repositories on 2026-10-04. See `docs/vision.md` for product
principles and `docs/next-steps-2026-10-04.md` for the verified state of the code._

## Strategic Thesis — updated 2026-10-04

RIM is a **programmable communication workspace for everyone**: gaming communities,
developer teams, friend groups, and ops teams alike. Chat is the substrate. The product is
what sits on top of it and is shared by every audience: a command palette, user-defined
keybinds, chainable macros, reactive pipelines, workspace state, capability-gated client
plugins, and a bring-your-own-model AI agent that operates the workspace through the same
command registry under a human confirmation gate.

The July 2026 roadmap bet the whole product on incident response for SRE teams. That built a
good vertical, but it narrowed the story to one buyer and started to shape the core around
that buyer (incident commands as the only AI-callable commands, docs and README written for
on-call teams). The correction:

1. **The runtime is the product; verticals are packs on top of it.** Incidents and on-call
   become the first *pack*: a bundle of commands, pipelines, macros, lenses and panels that a
   workspace turns on. A gaming community or a friend group never sees SEV levels. The same
   packaging primitive then ships a developer pack and a gaming pack. Packs are also the
   proof that the runtime is general: if a vertical needs a core change, the core was wrong.
2. **General-purpose chat parity is now a gate, not a distraction.** Private channels, real
   roles and moderation, notification rules, mobile reach and rich messages are what every
   audience expects before they notice the runtime. The July roadmap deferred these; this one
   front-loads them.
3. **Bots and webhooks are the developer story.** The server already models bot and webhook
   principals and has an incoming-webhook transport; finishing that (bot accounts, tokens,
   post-as-bot, outgoing webhooks, pipeline actions) serves developers directly and is what
   gaming communities build on too.
4. **The agent stays the differentiator, generalized.** Everyone else's AI summarizes; RIM's
   acts, under a gate, through the same registry a user's keybind uses. That is true for
   "catch me up on this channel and draft a reply" as much as for "run the failover runbook".
5. **Self-hosted, keyboard-first, non-Electron** stays the posture. Table stakes for the
   self-hosting segment; a quality attribute everywhere else.

**One sentence:** *RIM is the self-hosted chat workspace you program: commands, keybinds,
macros, pipelines, plugins and an AI operator, with packs for incidents, development and
gaming on top.*

## Where We Are Now

_Verified against code and GitHub on 2026-10-04 (both repositories)._

**Shipped and solid:**
- **Communication core**: workspaces, channels, DMs, group DMs, threads, edit/delete/forward,
  attachments and avatars with quotas, reactions, pins, mentions, read state, unread counts,
  inbox, mute, presence, typing, link previews, custom emoji, Postgres full-text search,
  channel lenses, invite-only registration with operator bootstrap, password reset, SMTP.
- **Programmable runtime**: command registry (~50 builtins, four surfaces, typed args,
  sensitivity tiers, per-command rate limits), user commands, macros with piping, 18 default
  keybinds with layers and overrides, workspace state (KV, TTL, atomic increment, events),
  pipelines v1 (six triggers, one action), capability registry with default-deny, audit log,
  confirmation proofs, skill-tier progressive disclosure, `command.discover`.
- **AI agent**: client-side, Anthropic and OpenAI, keys in the OS keyring, tool calls through
  the unified dispatcher, confirmation modal, audit emission, agent-visible command surface.
- **Plugin system (client)**: separate-process local host, manifest validation, capability
  broker with real enforcement, panels over a locked scheme, git install, trust badges,
  plugin AI consent.
- **Incident and on-call vertical**: incidents with phases, severity, timeline, action items,
  AI postmortem draft; on-call schedules, rotations, overrides; escalation policies with
  timers and boot recovery; alert webhooks (generic, Alertmanager, Grafana); incident and
  on-call commands AI-callable with timeline transcript.
- **Desktop shell**: side rail, workspace rail, tool panel, docked thread pane, status bar,
  spatial navigation, layout presets, zen mode, density modes, 15 settings tabs.
- **Quality**: server CI (fmt, clippy `-D warnings`, single-threaded tests, 75% line
  coverage gate with 95% target, audit, Docker); client CI (ESLint, Prettier, tsc, 1,348
  tests, build, Docker). Server type-checks and client builds and tests green on fresh clones.

**Known debt (carried, still open on 2026-10-04):**
- Client: three RCE-class desktop findings open (`rim-ui/fable-findings.md` S-H1..S-H3):
  unguarded terminal spawn, `git clone` with unvalidated URL, package-manager install before
  consent. The embedded terminal feature the April roadmap killed is still shipped.
- Client: reconnect refetch storm, hardcoded endpoint in several files, two error boundaries,
  `MessageItem.tsx` god file, no tests for pipelines/lenses/disclosure, two Czech labels.
- Server: no through-the-stack messaging e2e (paginator rework unverified end to end),
  resolver god files (`types.rs` 3.7k, `mutation.rs` 2.8k lines), audit retention job never
  scheduled, dead `rim_services/plugin_host` module, `cargo audit` advisory-only.
- Both: `rim-ui` `master` is still the initial commit; all code is on `feature/initial-code`.

## Competitive Snapshot — late 2026, general-purpose framing

| Who | What they own | Where RIM is different |
|---|---|---|
| **Discord** | Gaming communities, voice, rich presence, bots, roles | Self-hosted, programmable client, AI operator; voice is the gap (decision below) |
| **Slack** | Team chat UX, integrations, Workflow Builder, AI teammate | Open runtime instead of a closed workflow builder; your model, your box |
| **Mattermost / Rocket.Chat** | Self-hosted team chat, Playbooks, sovereign AI | Client-side plugins, keybinds, macros; packs instead of one vertical |
| **Element / Matrix** | Federation, E2EE, government | Not competing on federation or E2EE (anti-roadmap) |
| **Revolt / Guilded** | Open or indie Discord alternatives | Same audience overlap for gamers; RIM wins on programmability, loses on voice until decided |
| **Zulip** | Topic threading, mobile push | Threading parity not needed; their mobile reach is the bar |

## The Plan

Six tracks. Effort: S < 1w, M = 1–3w, L = 1–2mo, XL > 2mo. Tracks 0 and 1 come first;
the rest overlap. Every track lands as reviewable milestones with spec → implement → review.

### Track 0 — Safety and realignment (first, S/M total)

1. **Close the three desktop RCE findings** (M): gate terminal creation behind an explicit
   confirmation and a shell allowlist; validate plugin install URLs (scheme allowlist, no
   `ext::`, `--` separator, `GIT_ALLOW_PROTOCOL`); no `npm`/`pip` install without a consent
   step that names it as code execution.
2. **Terminal decision** (S): remove the embedded terminal from the product, or keep it as a
   capability-gated desktop feature. Default recommendation: remove the feature and its
   settings tab; keep the PTY code only if a plugin needs it later. See decisions.
3. **Pack toggle** (M): a per-workspace feature-pack setting that filters the command
   registry, settings tabs, side-rail entries and AI surface. Incidents and on-call become
   the `incidents` pack, off by default for new workspaces. This is the architectural change
   that makes "for everyone" true in the UI.
4. **Repo hygiene** (S): merge `rim-ui` `feature/initial-code` to `master`; make
   `cargo audit` blocking; close stale issues; schedule the audit retention job once the
   variant is picked; delete the dead server-side `plugin_host` module.
5. **Docs** (S): README, vision and this roadmap aligned (done with this rewrite); retire
   incident-first wording from onboarding copy.

### Track 1 — Chat for everyone (the parity gate, L total)

1. **Channels** (M): private channels with membership; channel browse and join/leave;
   channel ordering and categories; archive. Also guard the subscription visibility check,
   which today assumes every channel is visible to all workspace members.
2. **Roles and moderation** (M/L): a plain `Member` role between Moderator and Guest;
   per-channel permission overrides; kick, ban, timeout; delete others' messages; slow mode;
   message reports to moderators; moderation audit using the existing audit log.
3. **Notification rules** (M): per-conversation level (all, mentions, nothing), keyword
   alerts, do-not-disturb schedule, per-workspace defaults; delivery through desktop
   notifications now and push once Track 5 lands; email digest reusing the SMTP sender.
4. **Rich messages** (M): embeds for images, video and GIFs; code blocks with language
   hints; message quoting; polls; message content model on the server so clients agree on
   rendering (markdown stays the format, DOMPurify stays the backstop).
5. **People** (S/M): user search and directory inside a workspace; profile pages; status
   with expiry (exists) plus custom status emoji.
6. **Messaging e2e** (S): the through-the-stack pagination test from the review, landed with
   the first channel change so the paginator rework is finally verified.

### Track 2 — Bots, webhooks and the developer platform (L)

1. **Bot accounts** (M): create a bot in a workspace, issue and revoke tokens, post and read
   as the bot principal, bot avatar and badge. Unblocks the seeded system user that
   escalation and agent actions should author as.
2. **Incoming webhooks, generalized** (S/M): the alert receivers become one webhook
   transport with per-channel webhook URLs, templates, and a GitHub/GitLab/CI preset set.
3. **Outgoing webhooks and pipeline actions** (M): pipelines gain `PostMessage`,
   `CallWebhook`, `SetState`, and conditional branches; this is the "Slack Workflow Builder
   done right" story from the old roadmap, moved up because bots need it.
4. **Bot-registered commands** (M): a bot can register palette commands backed by its
   webhook or by a plugin, with the same typed args and sensitivity tiers as builtins.
5. **RIM CLI v1** (M): `rim send`, `rim search`, `rim watch`, `rim macro run`; doubles as
   the GraphQL contract test. Design exists in `docs/design/cli-v1-design-2026-04-22.md`.
6. **MCP bridge** (M): MCP servers as plugins and RIM as an MCP server, so external agents
   drive the same registry under `OutOfBand` confirmations. Sketch exists.

### Track 3 — Runtime depth (the moat, M/L each)

1. **Skill macros and intent routing**: macros as natural-language-routable units; the
   agent dispatches by intent with per-step `ai_callable` enforcement and rollup.
2. **Agentic macros (generalized runbooks)**: the agent walks a macro step by step, pausing
   at each Mutating/Dangerous step for a fresh human decision, with a visible transcript.
   The incident pack uses it as "runbooks"; a community uses it as "run the weekly event
   setup". The spec questions from the July handoff still apply (stepwise execution API,
   transcript target).
3. **Agent for everyday chat**: catch-me-up summaries over a channel or thread with
   citations (needs FTS, shipped), draft replies, find-and-link, schedule reminders. Parity
   with Slack AI and Mattermost Agents V2, but acting through commands.
4. **Shareable profiles**: export and import keybind sets, macros and pipelines as a
   profile ("gamer", "dev", "minimal"); the primitive packs reuse.
5. **Plugin marketplace v0** (after Track 0.1): git-backed curated manifest, signed; install
   from the palette. Economics deferred.
6. **Pipelines GA**: JSON transforms, error handling, run history and retries.

### Track 4 — Packs (verticals built only with Tracks 1–3 primitives)

1. **Incidents pack** (exists; S to finish): C5 incident metrics (MTTA/MTTR, history view),
   channel escalation-policy read query, `setMyTimezone`. Then maintenance only.
2. **Developer pack** (M): GitHub/GitLab webhook presets rendering PR, issue and CI cards;
   standup and release macros; `/pr`, `/issue`, `/deploy-status` commands; code-block
   first message rendering; CLI shortcuts.
3. **Gaming pack** (M): looking-for-group and party commands backed by workspace state;
   game status presence via a client plugin API; stream-live alerts via webhooks (Twitch,
   YouTube presets); event scheduling with reminders; scoreboard panels as plugin panels.
4. **Community pack** (S/M): welcome flows and auto-roles as pipelines, rules acceptance
   gate, announcement channels with reactions-only mode.

Each pack ships as a bundle definition (commands, pipelines, macros, lenses, panels) that
the pack toggle from Track 0.3 installs. A pack must not require a core schema change
beyond what Track 1 and 2 add; if it does, the gap goes to the core track, not the pack.

### Track 5 — Reach (M/L)

1. **Mobile PWA** (L): the existing web build made installable; push notifications via web
   push for mentions, DMs and pack events (the on-call page); narrow-layout shell per
   `docs/ui-shell.md` "Narrow Window Behavior". Native stays out of scope.
2. **Web client parity** (M): the capability matrix already gates features by platform;
   close the gaps that make the web build second class (keybinds, agent without the Rust
   proxy via a server-side relay option).
3. **Deployment** (S/M): one docker compose for server, client and Postgres; backup and
   restore runbook; first-run wizard replacing the CLI bootstrap for operators who want it.
4. **Internationalization and themes** (M): an i18n layer (the Czech labels show it is
   needed now), light theme, accessibility pass on the shell and composer.

### Voice — a decision, not a track yet

For gaming communities voice is table stakes; for everyone else it is a nice-to-have. The
July anti-roadmap ruled out building a native stack, and that still holds: media servers
are commodity. The open choice is whether to integrate a self-hosted open-source SFU
(LiveKit or similar) as voice rooms with the same self-hosted posture, delivered as a
first-party plugin so the core stays media-free. Recommendation: decide after Track 1,
prototype as a plugin during Track 3, and only then size it (likely L). See decisions.

## Anti-Roadmap — explicitly not building

- **A native media stack**: no own SFU, codecs or TURN; integrate if voice is approved.
- **Federation**: Matrix owns it; not what any of the target audiences ask for first.
- **E2EE as a headline**: conflicts with server-side search, AI grounding and pipelines. If
  it ships, DMs only.
- **Integration-directory breadth** and an enterprise agent platform.
- **Marketplace monetization** before there is a network.
- **Embedded terminal as a product feature** (pending the Track 0 decision).

## Sequencing (proposed)

| Phase | Weeks | Content |
|---|---|---|
| 1 | 1–6 | Track 0 complete; Track 1.1 channels, 1.2 roles/moderation, 1.6 e2e |
| 2 | 7–14 | Track 1.3 notifications, 1.4 rich messages, 1.5 people; Track 2.1 bots, 2.2 webhooks |
| 3 | 15–24 | Track 2.3 pipeline actions, 2.4 bot commands; Track 5.1 PWA + push; Track 3.1–3.3 agent depth; voice decision |
| 4 | 25+ | Track 4 developer and gaming packs; Track 2.5 CLI, 2.6 MCP; Track 3.4–3.6; Track 5.2–5.4 |

The incidents pack is finished (Track 4.1) whenever an S-sized slot opens; it is not on the
critical path.

## Open Decisions (for Jakub)

New with this roadmap:
1. **Ratify the general-purpose direction** as written here (packs on the runtime, parity
   first, incidents demoted to a pack).
2. **Voice**: integrate a self-hosted SFU as a plugin (recommended, timing per above), or
   stay text-only for now.
3. **Embedded terminal**: remove (recommended) or keep capability-gated.
4. **Incidents pack default**: off for new workspaces (recommended) or on.
5. **Roles model depth**: a simple role ladder with per-channel overrides (recommended), or
   Discord-style arbitrary roles with permission bitfields from the start.
6. **Mobile**: PWA-first (recommended) or native.

Carried, still unratified:
7. Audit retention: Variant A (90d/365d) vs B (365/365); the job is unscheduled until then.
8. `oncall.take_shift` authorization: self-service or owner/moderator gated.
9. `.env` history purge: run the runbook or skip given rotation.
10. Plugin reframe to client-side architecture: ratify formally (it is the de facto state).

## Ledger — what the previous roadmaps shipped

For history. Phases 0–4 of the April 2026 roadmap (communication core, command system,
keybinds, plugins, incident mode v1, AI agent reframe phases 1–5) and Tracks A, B, C1, C2
of the July 2026 roadmap (foundations, search/alerts/email, on-call and escalation,
agent-operator fusion) are all merged on `rim-server` `master` (PR #11, 2026-10-04) and on
`rim-ui` `feature/initial-code` (`d907c86`). Point-in-time specs live in `docs/specs/`,
`docs/design/`, `docs/plans/` (rim-ui) and `docs/research/`.
