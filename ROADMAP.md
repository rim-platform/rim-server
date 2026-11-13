# Roadmap

Where RIM is heading. `docs/vision.md` says what RIM is for and the principles behind every item
here; `CHANGELOG.md` says what each release changed. The roadmap names directions and their
order, not dates. Each item becomes a pull request with its contract in `docs/` first, so the
detail lives there once the work starts.

## Where it stands

RIM is **pre-release**. Early `0.0.x` releases of the server and clients are available, and the
public relay at `relay.rim-platform.org` is running as a first community test. "What works today"
below describes the current `master` branches; a release may not include every recent change.

The `0.0.x` releases are **pre-review**: nobody outside the project has checked the cryptography
of key identity, device linking, recovery and end-to-end encryption yet. `0.1.0`, and any claim
that RIM is ready for production, waits for that outside review (`docs/security-overview.md`).

### What works today

| Area | |
|---|---|
| Chat | Workspaces, public and private channels, DMs and groups, threads, replies, edits, forwards, attachments with quotas, reactions, pins, mentions, read state with mark unread, an inbox, a Home page built from boxes, an Unreads page across workspaces, notification rules with an email digest, presence and typing, link previews, custom emoji, full-text search |
| Moderation | Owner, Moderator, Member and Guest; kick, ban, timeout, slow mode, message reports including franked reports of encrypted messages; an audit log with retention |
| The programmable layer | A command registry with typed arguments, sensitivity tiers and confirmations, where every button is also a command for the palette, a keybind, a macro or a Home box; refusals that say why a command did not run; macros kept with your account; keybinds with layers and an opt-in Vim profile; pipelines that can welcome people as a bot; workspace state; channel lenses |
| Plugins | Local-process plugins on the desktop and sandboxed web plugins on the desktop and the web; a TypeScript SDK and a frozen protocol; per-workspace registries of publisher-signed plugins; plugins that run the app's own commands a person grants them; four first-party plugins (Kubernetes pods, GitHub pull requests, polls, releases) |
| AI agents | A built-in agent with your own Anthropic or OpenAI key, whose changes wait for confirmation. Agents on your computer, such as Claude Code, connect to the desktop app: they read only the channels you grant, every send or change waits for your approval, and a run view shows what each was given. The desktop app can also install and start agent programs (Claude Code, Codex, Gemini CLI, opencode, Goose) in a folder and direct them from RIM. The contract is `docs/specs/agent-data-flow-v1.md` |
| Bots and webhooks | Bot accounts an Owner or Moderator creates, with revocable tokens and a small REST API; one incoming webhook URL per channel, bound to a bot, with formats anyone can write and ready-made ones for GitHub and Grafana |
| Identity | Accounts are key pairs: identity and device keys, an OPAQUE account password, device linking by code, a recovery phrase and an identity-key backup |
| Encryption | MLS for DMs and private groups with a hybrid X25519 + ML-KEM-768 key agreement; channels stay plaintext |
| Decentralization | Every server has its own key; relays with a request inbox for strangers; workspace hosts that serve members from other servers through their relay |
| History on your devices | An encrypted local message cache with local search and offline reading; an encrypted archive in a folder, Google Drive, Dropbox, OneDrive, S3-compatible storage or a separate backup endpoint |
| Desktop | Linux and Windows apps with signed updates; staying in the tray and starting at login, both opt-in |
| Operations | A Docker Compose stack with backup, restore and a restore drill; health probes, Prometheus metrics, instance feature switches, an upgrade guide with rollback |

## Now: people using it

The features for daily use are in place. Until a few teams use RIM every day, the work is making
it easy to start and reliable to keep open, not adding features.

- Have two or three small teams outside the project use it for their real work, and fix what
  they trip on first, so decisions rest on more than one person's use.
- Make the first minutes short: a new person joins a workspace, exchanges messages and tries one
  plugin, webhook or agent without help.
- Move RIM's own development into a RIM workspace, so every feature is used daily.
- Maintain the early `0.0.x` releases through the release checklist
  (`docs/operations/release-checklist.md`): signed server images and desktop builds for Linux
  and Windows, with feedback from people trying them.
- Run the public relay `relay.rim-platform.org` as a first test run, hosting the RIM community
  workspace every new account joins, with the web client on `chat.rim-platform.org`
  (`docs/operations/relay-public.md`).
- Make the four first-party plugins and their signed release bundles easy to discover and install.
- Start scheduling the outside cryptographic review.

## Next

**The outside cryptographic review**, then `0.1.0`. Before it: a "why can't I read this" view that
explains an undecryptable message, and a manual multi-device test pass in the release checklist.
The review runs on its own track: it does not wait for the items below, and if they slip, `0.1.0`
does not.

Refusals that say why, the contract for where agents may move data, RIM in Claude Code and other
agents, and bots with incoming webhooks shipped in server `0.0.9` and the clients `0.0.12`; they
are under "What works today".

In order, once the work in "Now" calls for them:

1. **A first-run path** in the clients that grows with the product: create a key and write down
   the phrase, choose a drive for the archive, pick a relay, install a first plugin.
2. **Data export** per person and per workspace.
3. **Outgoing webhooks and richer pipelines**: posting, calling a webhook and setting state as
   pipeline actions, conditions, run history and retries; commands a bot registers in the
   palette.
4. **Member lists at relay scale**, so a workspace with thousands of members does not load them
   all at once, and handle lookup for `@username:rim-platform.org`.

## Later

Not scheduled; picked up when the items above are done or when someone needs them enough to
build them.

- **Agent work the team can see:** a run card shared in a conversation, picked up only if daily use
  shows the need. It never reveals what the agent read somewhere more private.
- **Approvals by another person:** a person with authority approves what a bot asked for, with
  stored run records, picked up after the agent features have been in daily use.
- **Cards on Home:** plugins adding their own cards to Home, once the SDK has a way to.
- **Calls:** voice calls with the team, and video and screen sharing after them, because talking
  a thing through is sometimes better than writing it. They follow the same rules as messages:
  calls in DMs and private groups are end-to-end encrypted, and a self-hosted server can carry
  them without a third-party service. Whether the media runs through the server or a separate
  self-hosted media server is decided when the work starts.
- **The agent:** local and self-hosted models first (Ollama and OpenAI-compatible endpoints); macros
  the agent runs step by step with a pause at each change; catch-me-up summaries with citations,
  acting through commands rather than as a separate chatbot.
- **More MCP:** MCP servers installed as plugins, so the built-in agent can use them; and a
  headless MCP server with a bot token, for agents that run without a person at a desktop. These
  follow the contract from "Next", and on the public relay they stay on your own machine.
- **`rim-cli`:** `rim send`, `rim search`, `rim watch` and `rim macro run` with a host-local
  token; it doubles as a contract test of the GraphQL API.
- **Reach:** the web app as an installable PWA with web push and a phone layout; narrow windows
  work today, with nothing built for touch. No native mobile app; the cache and encryption code is Rust a future mobile build can reuse.
- **Post-quantum signatures:** Ed25519 + ML-DSA-44 for identity and device keys, profile cards,
  login proofs, MLS credentials and server-to-server signatures, after the review. The formats
  already carry an algorithm tag and room for the larger keys.
- **Deeper plugin surfaces:** plugin context actions on messages and members, plugin access to
  workspace state and pipeline events, plugin-provided lenses, and a curated plugin index on
  top of the signed registries.
- **Keyboard and polish:** macros and pipelines in shareable profiles, fuzzy jump to anything,
  a light theme, translations and an accessibility pass.
- **Operators:** admin audit views and a pass over what is logged and kept, documented in
  `docs/security/`.

## Not planned

- **Feature parity with Discord or Slack.** Chat in the core stays bounded on purpose; calls
  aim at a team talking things through, not at large live audiences.
- **First-party verticals.** No gaming pack, no developer pack, no incident tracker in the core;
  showcase plugins instead.
- **Encrypted channels.** Channels stay plaintext so server search, pipelines and the agent keep
  working there; end-to-end encryption covers DMs and private groups.
- **Domain federation, Matrix-style room replication, peer-to-peer.** A person is a key, not
  `name@domain`; a domain may return only as an optional alias on top of a key
  (`docs/specs/key-identity-v1.md`).
- **A central RIM service** anyone depends on, and marketplace monetization before there is a
  network of plugins.
- **A replacement for your project tracker, calendar or CI.** RIM shows what needs you and links to
  the source.
- **A model service for agents run by the RIM project.** No agent's prompts or model key go through
  a service the project runs; a server's owner decides what runs on their own server.

## Proposing a change to the roadmap

Open an issue in `rim-server` with the feature request template. It asks what you want, for
whom, and whether it belongs in the core or in a plugin; `CONTRIBUTING.md` ("What belongs in the
core") says where that line is.
