# What RIM is for

`ROADMAP.md` says where RIM is heading; this page says what it is, who it is for and the
principles every change is held to.

## In one sentence

> RIM is the self-hosted communication workspace you program: commands, keybinds, macros,
> pipelines, plugins and an AI operator on top of chat, private and decentralized.

Chat is the medium. The product is what people build on top of it, the way VS Code turned a
text editor into a platform with a command palette and extensions, or Obsidian turned a folder
of notes into whatever its plugins make of it. A team, a community or a group of friends shapes
RIM around how it already works, instead of adapting to the tool.

## What people use it for

- **A developer or ops team.** Chat next to the systems the team runs. The Kubernetes pods
  panel lists the pods of the cluster in your kubeconfig beside the conversation, the GitHub
  panel shows the pull requests waiting for you, and a keybind or the AI agent restarts a
  crashlooping pod after you confirm it. Every action is a command, so a common sequence becomes
  one keystroke or a macro.
- **An open-source community.** A workspace on a server the project runs, or hosted for it on
  someone else's, with public channels, roles and moderation, and plugins a moderator approved
  in a signed workspace registry. Release posts, polls and the project's own commands come from
  plugins and pipelines rather than from features in RIM.
- **A private group: friends, a family, a small association.** One box you control, DMs and
  private groups end-to-end encrypted, and accounts that are keys rather than email addresses.
  If the server goes away, people move to another relay and keep their identity; their history
  comes back from the encrypted archive on their own drive.
- **Anyone who lives in a chat client all day and prefers the keyboard.** A palette over every
  action, keybinds for anything, an opt-in Vim layer, and a desktop app that is not a browser in
  a box.

These start from the same core. RIM does not ship a feature per audience; each of them adds
what it needs through commands, keybinds, macros, pipelines and plugins.

## Who it is not for, yet or ever

- **Voice and video.** RIM does not build them. A community that needs calls uses a
  dedicated tool, or a bridge plugin to a self-hosted media server once one exists.
- **Phones first.** There is a desktop app and a web app; there is no mobile app, and the web
  app has no narrow layout yet.
- **An enterprise checklist.** There is no compliance suite and no directory of thousands of
  integrations, and neither is planned in the core.
- **The simplest possible chat.** RIM works without any customization, but its reason to exist
  is that you can program it. If you never will, a simpler tool suits you better.
- **Anything that needs audited cryptography today.** The 0.0.x releases come out before the
  outside review of the cryptography (`docs/security-overview.md`).

## What makes it different

Most of RIM's parts exist somewhere: Slack ships AI agents, Mattermost and Zulip run on your own
server, Mattermost has client plugins, Element encrypts end to end, Discord lets you rebind
keys. A feature checklist ages with every release of every product. What RIM claims is these
together, in a workspace its community runs and programs itself:

| | What it means in RIM |
|---|---|
| **One command registry** | Every action is a command with typed arguments and a sensitivity tier. The palette, keybinds, macros, pipelines, plugins and the AI agent all call the same registry, so a plugin's command is at once something you can bind, chain and ask the agent to do. |
| **Plugins that extend the client** | A plugin adds panels, commands, context actions and keybinds to the application itself, under capability prompts, not just a bot that posts into a channel. A local plugin runs next to you with your own credentials, which is how a Kubernetes panel can exist at all. |
| **An AI agent that acts** | Bring your own provider and key. The agent works through the command registry and nothing else; anything that changes something waits for a person to confirm it. Because plugins register commands, the agent can operate whatever a workspace has plugged in. |
| **Self-hosted and decentralized** | A person is a key pair, not an account on one server. Any server can be a person's relay or a workspace's host, and none is central. DMs and private groups are end-to-end encrypted with MLS; channels stay plaintext so server search, pipelines and the agent keep working there. |

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
