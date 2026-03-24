# RIM — Product Vision

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

5. **Tauri v2 desktop runtime** — significantly smaller binary, lower memory usage, and
   better native OS integration compared to Electron-based competitors.

6. **Future E2EE** — planned end-to-end encryption for text messages, which Slack and
   Discord explicitly refuse to implement.

## Target Users

RIM is built for people who:

- Spend most of their day in a communication tool and want it to be fast.
- Prefer keyboard over mouse.
- Want to customize their tools (keybinds, macros, workflows).
- Value extensibility and are willing to install or build plugins.
- Care about performance and resource efficiency.
- Work in technical teams (developers, DevOps, security, research).

RIM is not built for people who want the simplest possible chat experience or who
need an enterprise platform with thousands of pre-built integrations.

## Competitive Landscape

See `docs/competitive-analysis.md` for the full analysis.

Summary: No competing product combines a command palette, custom keybinds, macro
commands, client-side plugins, and a non-Electron desktop runtime. Each competitor
has one or two of these, but none has all of them.

| Feature               | Slack | Discord | Mattermost | Zulip | Element | RIM   |
|-----------------------|-------|---------|------------|-------|---------|-------|
| Command palette       | ❌    | ❌      | ❌         | ❌    | ❌      | ✅    |
| Custom keybinds       | ❌    | 🔧     | ❌         | ❌    | ❌      | ✅    |
| Macro commands        | ❌    | ❌      | ❌         | ❌    | ❌      | ✅    |
| Client-side plugins   | ❌    | ❌      | ✅         | ❌    | ❌      | ✅    |
| Non-Electron desktop  | ❌    | ❌      | ❌         | ❌    | ❌      | ✅    |
| Text E2EE             | ❌    | ❌      | ❌         | ❌    | ✅      | 🔜   |

## Product Positioning — One Sentence

> RIM is a programmable communication workspace where users build their own workflows
> instead of adapting to the platform.

## Inspirations

- **Superhuman** — command palette UX in a communication app
  (https://blog.superhuman.com/how-to-build-a-remarkable-command-palette/)
- **Linear** — keyboard-first navigation with G-prefixed shortcuts
- **VS Code** — command palette + extensions as a platform strategy
- **Obsidian** — community plugin ecosystem that defines the product experience
- **Raycast** — instant keyboard-driven access to everything
