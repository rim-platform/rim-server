# RIM — Product Vision

_The roadmap (`ROADMAP.md`) owns sequencing; this document owns the product principles._

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

## Who RIM Is For

RIM is not trying to replace Discord, Slack or Matrix. It is for people who want something
those platforms do not offer: a communication workspace they can program. Gaming communities,
developer teams, friend groups and ops teams all fit, and each shapes the workspace
differently through commands, keybinds, macros, pipelines and plugins, without RIM shipping a
feature for each of them. A developer adds a panel that shows the pods in their Kubernetes
cluster; a community adds its own commands and automations; a privacy-minded group runs it on
their own box.

Pitch: *the self-hosted communication workspace you program.* A bring-your-own-model AI agent
acts through the same command registry under a human confirmation gate, so it can operate
whatever a workspace has plugged in. Other products' AI suggests; RIM's acts.

Two commitments shape the architecture: **end-to-end encryption** for DMs and private groups,
and **decentralization**, where a person is a key pair and servers are interchangeable relays and
workspace hosts. Both are built; `ROADMAP.md` carries the guardrails that keep new features
compatible with them.

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

3. **User commands and macros** — users compose multi-step workflows from individual
   commands. A user command bound to a keystroke runs all its steps; a macro runs its
   steps from Settings or `/macro` in the composer. No other team chat app offers this.

4. **Client-side plugin system** — plugins extend the application itself (new panels,
   commands, context actions, behaviors), not just post messages into channels.
   Plugins are first-class citizens with capability-based permissions, trust boundaries
   and per-workspace registries of signed plugins.

5. **An AI operator, not an AI summarizer** — a bring-your-own-model agent drives the
   workspace through the command registry, with capability checks and per-action human
   confirmation for anything mutating or dangerous. Because plugins register commands in the
   same registry, the agent can operate whatever a workspace has plugged in.

6. **Self-hosted, auditable, private** — one box, your data, your model, your audit log;
   end-to-end encrypted DMs and private groups; no central server, since any server can be a
   person's relay or a workspace's host.

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
- Run a community or a team and want to shape the tool around it: gamers, developers,
  ops teams, friend groups.
- Prefer to self-host their communication, or at least to be able to.

RIM is not built for people who want the simplest possible chat experience or who
need an enterprise platform with thousands of pre-built integrations.

## Competitive Landscape

No competing team-chat product combines a command palette, custom keybinds,
macro commands, and client-side plugins.

| Feature               | Slack | Discord | Mattermost | Zulip | Element | RIM   |
|-----------------------|-------|---------|------------|-------|---------|-------|
| Command palette       | ❌    | ❌      | ❌         | ❌    | ❌      | ✅    |
| Custom keybinds       | ❌    | 🔧     | ❌         | ❌    | ❌      | ✅    |
| Macro commands        | ❌    | ❌      | ❌         | ❌    | ❌      | ✅    |
| Client-side plugins   | ❌    | ❌      | ✅         | ❌    | ❌      | ✅    |
| Non-Electron desktop  | ❌    | ❌      | ❌         | ❌    | ❌      | ✅    |
| Text E2EE             | ❌    | ❌      | ❌         | ❌    | ✅      | ✅ *  |

\* DMs and private groups, with MLS (`docs/specs/e2ee-mls-v1.md`). Channels stay plaintext so
server search, pipelines and AI grounding keep working there; in encrypted conversations those
run on the client (see the architecture guardrails in `ROADMAP.md`).

## Product Positioning — One Sentence

> RIM is the self-hosted communication workspace you program: commands, keybinds, macros,
> pipelines, plugins and an AI operator on top of chat, private and decentralized.

## Inspirations

- **Superhuman** — command palette UX in a communication app
  (https://blog.superhuman.com/how-to-build-a-remarkable-command-palette/)
- **Linear** — keyboard-first navigation with G-prefixed shortcuts
- **VS Code** — command palette + extensions as a platform strategy
- **Obsidian** — community plugin ecosystem that defines the product experience
- **Raycast** — instant keyboard-driven access to everything
