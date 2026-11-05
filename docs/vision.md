# What RIM is for

`ROADMAP.md` says where RIM is heading; this page says what RIM is, why it exists and the
principles every change is held to.

## In one sentence

> RIM is the self-hosted communication workspace you program: commands, keybinds, macros,
> pipelines, plugins and an AI operator on top of chat, private and decentralized.

RIM is a place for a team's daily work. It shows what needs you and links to the source, and it
does not try to replace the project tracker, the calendar or CI. Agents are optional, and RIM
works the same without them.

## Your own workspace

RIM is for teams, organizations and communities that want a workspace of their own: built
around how they work, extended with their own plugins, and running where they choose, with
their data under their control.

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
  person to confirm a built-in change. A plugin command runs without asking only if you approved
  it for the agent, and a dangerous one is only proposed.

RIM does not ship a feature per kind of group. A development team, a company, an open-source
community and an association start from the same core, and each adds what it needs.

## Working with agents

These are the rules every agent feature is held to; `ROADMAP.md` says which exist yet.

- **Optional.** A workspace can use RIM as plain chat.
- **Whoever runs an agent decides where.** They also choose its model or key. The RIM project runs
  no model service for agents, and no agent's prompts go through one. Every agent reaches RIM as a
  client, with the same limits and checks as anyone else.
- **Your own agent runs on your machine.** It talks directly to the provider you chose.
- **A server's owner may run an agent on that server.** It is their choice, and it is off until
  they turn it on. It can read only what the server can already read, never an encrypted
  conversation, and people on that server can see that it runs, who runs it and where what it
  reads goes. The content check in `docs/self-hosting.md` shows the pattern: it runs next to the
  server, what it scores leaves the host no further, and the owner can switch it off. It is a
  scoring service rather than an agent.
- **No more than the person it acts for.** An agent is further limited by what that person
  granted. It may ask for more, but it never grants itself anything or confirms its own actions.
- **You decide what your agent can read, and RIM shows what it was given.** Nothing private reaches
  it without your grant, and a read you granted does not prompt each time.
- **RIM stops at the handover.** Once an agent has content, where it goes depends on the agent and
  model you chose. A local model keeps it on your machine; a cloud provider receives it. RIM cannot
  see or limit that step.
- **A built-in change waits for a person.** A confirmation covers one call with its exact arguments
  and expires.
- **Plugin commands are approved one by one.** An agent gets only the commands a plugin offers to
  agents and you approve for that agent. An outside agent still asks before an approved command
  that changes something. A dangerous command is only proposed for you to run.
- **A bot has its own identity.** It has a token an Owner or Moderator can revoke, and whoever runs
  it chooses where, with their own model and key. It acts within its grants without a confirmation,
  and the people in a channel can see who runs it and where what it reads goes. Bots never join
  encrypted conversations.
- **A refusal says why.** Denied by policy, waiting for a confirmation, failed, cancelled and
  expired are different outcomes and look different.
- **A run keeps its scope.** It keeps the workspace and permissions it started with.
- **Encrypted conversations stay closed.** An agent reads one only if a person grants that one
  conversation, and the others in it can see that they did. That notice comes from each person's
  app: it cannot stop a modified client or someone copying text by hand.

## Who it is for first

Small development teams and open-source projects that run their own server come first, because
RIM is built that way itself. Companies, associations and communities use the same core. A
community can offer no bots or agent features and use RIM as plain chat, though that does not stop
a member's own agent from reading what that member can read. This names an audience, not a
vertical: "No first-party verticals" under "Principles" still holds.

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
points of "Your own workspace" together. One command registry is shared by the palette, keybinds,
macros, pipelines, plugins and the agent, so a plugin's command is at once something you can bind,
chain and ask the agent to do. Plugins extend the client rather than post into a channel, and a
local plugin runs next to you with your own credentials, which is how a Kubernetes panel can
exist at all. And the workspace runs on a server you choose, with no central server behind it.

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
- **Agents are accountable.** What an agent may do is visible before it acts, what RIM gave it and
  what it did in RIM are visible after, and it can never approve itself.
- **Prove it on ourselves.** Each platform capability ships with something real built on it:
  the pods panel, the pull-request panel, the poll and release plugins.
