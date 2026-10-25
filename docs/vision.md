# What RIM is for

`ROADMAP.md` says where RIM is heading; this page says what RIM is, why it exists and the
principles every change is held to.

## In one sentence

> RIM is the self-hosted communication workspace you program: commands, keybinds, macros,
> pipelines, plugins and an AI operator on top of chat, private and decentralized.

## Your own workspace

RIM is for teams and organizations that want a workspace of their own: built around how they
work, extended with their own plugins, and running where they choose, with their data under
their control.

Chat is the medium. The product is what you build on top of it, the way VS Code turned a text
editor into a platform with a command palette and extensions. Instead of adapting to a tool
someone else designed, you shape RIM around your work:

- **Built your way.** Every action is a command. Bind any of them to a key, chain them into
  macros, and let pipelines run them when something happens. The workspace learns your
  workflows instead of you learning its menus.
- **Extended with your own plugins.** A plugin adds panels, commands and actions to the app
  itself, so the tools you work with sit next to the conversation about them: the pods running
  in your cluster, the pull requests waiting for review, your own internal systems. An
  organization keeps a signed registry of the plugins its members may install.
- **Your data, under your control.** You run the server, on your own hardware or wherever you
  choose; nothing depends on a service run by RIM. Direct messages and private groups are
  end-to-end encrypted, so not even the server's operator can read them. People are key pairs,
  not accounts on someone's server, and their history is kept encrypted on their own devices and
  drives.
- **An AI assistant on your terms.** Bring your own model provider and key. The agent works only
  through the same commands you use, can operate whatever your plugins add, and waits for a
  person to confirm anything that changes something.

RIM does not ship a feature per kind of team. A development team, an open-source project, a
company department and an association start from the same core, and each adds what it needs.

## What it does not do yet

- **Calls.** Talking a thing through is sometimes better than writing it, so voice and video
  calls are on the roadmap (`ROADMAP.md`, "Later"). Until then a team uses a separate tool for
  them.
- **A mobile app.** There is a desktop app and a web app; the web app has no phone layout yet.
- **Audited cryptography.** The `0.0.x` releases come out before the outside review of the
  cryptography (`docs/security-overview.md`).

## What makes it different

Most of RIM's parts exist somewhere: Slack ships AI agents, Mattermost and Zulip run on your own
server, Mattermost has client plugins, Element encrypts end to end, Discord lets you rebind
keys. A feature checklist ages with every release of every product. What RIM offers is the four
above together. One command registry is shared by the palette, keybinds, macros, pipelines,
plugins and the agent, so a plugin's command is at once something you can bind, chain and ask
the agent to do. Plugins extend the client rather than post into a channel, and a local plugin
runs next to you with your own credentials, which is how a Kubernetes panel can exist at all.
And the workspace runs on a server you choose, with no central server behind it.

## Principles

The rules a change is measured against. `CONTRIBUTING.md` turns them into what the core accepts,
and `docs/architecture.md` into the design guardrails.

- **A platform, not a pile of features.** When a plugin, a bot, a pipeline or a macro can meet a
  need, the core gains the primitive that makes it possible and an example built on it, not the
  feature. No first-party verticals: no gaming pack, no developer pack, no incident tracker in
  the core.
- **Enough chat, not parity chat.** A chat feature belongs in the core when people cannot use
  RIM every day without it. The bar is comfort, not Discord or Slack.
- **Keyboard first.** Every surface is reachable without a mouse, every command can be bound,
  and the modal Vim layer is there for those who want it, off by default.
- **Private by design.** A server feature may benefit from reading message text but may never
  require it, because encrypted conversations have none to read. Plugins see decrypted messages
  only through a grant the person gave that plugin.
- **No central server.** Nothing depends on a RIM-run service. The public relay is a
  convenience for trying RIM, and leaving it costs nothing but a move.
- **Capabilities are visible.** Running a local process, reaching the network or the
  filesystem, acting as you: each is asked for in words that say exactly what is granted, and
  nothing gains power just because it is installed.
- **Prove it on ourselves.** Each platform capability ships with something real built on it:
  the pods panel, the pull-request panel, the poll and release plugins.
