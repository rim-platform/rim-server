# RIM — Product Vision

_Last updated 2026-10-03 to match the incident-wedge strategy ratified in `ROADMAP.md`
(2026-07-10). The roadmap owns sequencing; this document owns the product principles._

## What RIM Is

RIM is a programmable communication workspace.

It is not another chat app. Chat is the medium through which work happens, but the
product is defined by what users can do with that communication context — commands,
keybinds, macros, plugins, and context actions that turn conversations into workflows.

The closest analogies are not Slack or Discord. They are:

- VS Code — a command palette, keybinds, and extensions turned a text editor into a
  platform for developer workflows.
- Raycast / Alfred — a keyboard-first launcher that makes every action instant.
- Obsidian — an extensible engine where plugins define the experience.

RIM applies the same philosophy to team communication: the user builds their own
workflow instead of adapting to the platform.

## Where RIM Goes to Market

A programmable workspace is a platform, and platforms need a first job to do. RIM's is
**incident response for self-hosting dev/SRE/platform teams**: alerts arrive, an incident is
declared, on-call is paged and escalated, and responders — and an AI agent — work the
incident through the same commands, macros, and pipelines as everything else.

Pitch: *Slack + incident.io in one self-hosted box, with an AI agent that can actually run
the incident.* Incident vendors rent someone else's chat and can only *suggest* actions;
RIM owns the substrate, so its agent *acts* — through the command registry, under a human
confirmation gate. The programmable runtime is what makes that possible and hard to copy.

## What RIM Is Not

- RIM is not a Slack/Discord clone with extra features.
- RIM is not trying to win on breadth of integrations or number of users.
- RIM does not compete on having the best chat UX — it competes on having the most
  programmable and keyboard-efficient communication environment.
- RIM is not a platform that dictates how teams work — it is a tool that adapts to how
  they already work.

## Core Differentiators

These are the capabilities that no competing product offers together:

1. **Command palette** — a universal, fuzzy-searchable action launcher for every
   operation in the app. Not a channel switcher. Not slash commands. A true command
   palette like VS Code's Ctrl+Shift+P.

2. **User-defined keybinds** — any action can be bound to any key combination,
   scoped by context (global, composer, message list, thread view, panel). No other
   team chat app allows this.

3. **Chainable macro commands** — users can compose multi-step workflows from
   individual commands. One keystroke, multiple actions. No other team chat app offers
   this.

4. **Client-side plugin system** — plugins extend the application itself (new panels,
   commands, context actions, behaviors), not just post messages into channels.
   Plugins are first-class citizens with a marketplace, capability-based permissions,
   and trust boundaries.

5. **Native incident layer with an AI operator** — incidents, on-call schedules,
   escalation policies, and alert ingestion are first-class objects in the workspace, not a
   bot bolted onto someone else's chat. A bring-your-own-model agent drives them through the
   command registry, with capability checks and per-action human confirmation for anything
   mutating or dangerous.

6. **Self-hosted and sovereign** — one box, your data, your model. Table stakes in this
   segment rather than a differentiator on its own, but required by the buyers RIM targets.

The Tauri v2 desktop runtime (smaller binary, lower memory than Electron) is a real
engineering advantage, but users rarely choose a tool for it — treat it as a quality
attribute, not a headline.

## Target Users

RIM is built for people who:

- Spend most of their day in a communication tool and want it to be fast.
- Prefer keyboard over mouse.
- Want to customize their tools (keybinds, macros, workflows).
- Value extensibility and are willing to install or build plugins.
- Care about performance and resource efficiency.
- Work in technical teams (developers, DevOps, SRE, platform, security).
- Run their own infrastructure and want their communication and incident tooling there too.

RIM is not built for people who want the simplest possible chat experience or who
need an enterprise platform with thousands of pre-built integrations.

## Competitive Landscape

See `docs/competitive-analysis.md` for the full analysis.

Summary: No competing team-chat product combines a command palette, custom keybinds,
macro commands, and client-side plugins. On the incident side, Mattermost (the closest
competitor) has Playbooks checklists and Agents V2 but delegates on-call and escalation to
PagerDuty/Opsgenie, and its agents cannot drive runbooks. See
`docs/research/competitive-landscape-2026-04-29.md` and
`docs/research/mattermost-playbooks-teardown-2026-07-10.md`.

| Feature               | Slack | Discord | Mattermost | Zulip | Element | RIM   |
|-----------------------|-------|---------|------------|-------|---------|-------|
| Command palette       | ❌    | ❌      | ❌         | ❌    | ❌      | ✅    |
| Custom keybinds       | ❌    | 🔧     | ❌         | ❌    | ❌      | ✅    |
| Macro commands        | ❌    | ❌      | ❌         | ❌    | ❌      | ✅    |
| Client-side plugins   | ❌    | ❌      | ✅         | ❌    | ❌      | ✅    |
| Non-Electron desktop  | ❌    | ❌      | ❌         | ❌    | ❌      | ✅    |
| Text E2EE             | ❌    | ❌      | ❌         | ❌    | ✅      | ❌ *  |

\* Deliberately not a headline: end-to-end encryption conflicts with server-side search, AI
grounding, and incident indexing. If it ships, it is scoped to DMs (see the anti-roadmap in
`ROADMAP.md`).

## Product Positioning — One Sentence

> RIM is a self-hosted, programmable communication workspace where teams build their own
> workflows — starting with incident response run by people and an AI agent together.

## Inspirations

- **Superhuman** — command palette UX in a communication app
  (https://blog.superhuman.com/how-to-build-a-remarkable-command-palette/)
- **Linear** — keyboard-first navigation with G-prefixed shortcuts
- **VS Code** — command palette + extensions as a platform strategy
- **Obsidian** — community plugin ecosystem that defines the product experience
- **Raycast** — instant keyboard-driven access to everything
