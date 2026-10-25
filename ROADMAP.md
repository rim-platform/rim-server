# Roadmap

Where RIM is heading. `docs/vision.md` says what RIM is for and the principles behind every item
here; `CHANGELOG.md` says what each release changed. The roadmap names directions and their
order, not dates. Each item becomes a pull request with its contract in `docs/` first, so the
detail lives there once the work starts.

## Where it stands

RIM is **pre-release**. Everything in "What works today" below runs on both `master` branches,
and the first `0.0.x` releases of the server and the clients are being prepared together with the
public relay at `relay.rim-platform.org`.

The `0.0.x` releases are **pre-review**: nobody outside the project has checked the cryptography
of key identity, device linking, recovery and end-to-end encryption yet. `0.1.0`, and any claim
that RIM is ready for production, waits for that outside review (`docs/security-overview.md`).

### What works today

| Area | |
|---|---|
| Chat | Workspaces, public and private channels, DMs and groups, threads, replies, edits, forwards, attachments with quotas, reactions, pins, mentions, read state, an inbox, notification rules with an email digest, presence and typing, link previews, custom emoji, full-text search |
| Moderation | Owner, Moderator, Member and Guest; kick, ban, timeout, slow mode, message reports including franked reports of encrypted messages; an audit log with retention |
| The programmable layer | A command registry with typed arguments, sensitivity tiers and confirmations; user commands and macros; keybinds with layers and an opt-in Vim profile; pipelines; workspace state; channel lenses |
| Plugins | Local-process plugins on the desktop and sandboxed web plugins on the desktop and the web; a TypeScript SDK and a frozen protocol; per-workspace registries of publisher-signed plugins; four first-party plugins (Kubernetes pods, GitHub pull requests, polls, releases) |
| AI agent | Anthropic or OpenAI with your own key; it calls the built-in and plugin commands, and every change it makes waits for a confirmation |
| Identity | Accounts are key pairs: identity and device keys, an OPAQUE account password, device linking by code, a recovery phrase and an identity-key backup |
| Encryption | MLS for DMs and private groups with a hybrid X25519 + ML-KEM-768 key agreement; channels stay plaintext |
| Decentralization | Every server has its own key; relays with a request inbox for strangers; workspace hosts that serve members from other servers through their relay |
| History on your devices | An encrypted local message cache with local search and offline reading; an encrypted archive in a folder, Google Drive or Dropbox |
| Operations | A Docker Compose stack with backup, restore and a restore drill; health probes, Prometheus metrics, instance feature switches, an upgrade guide with rollback |

## Now: the first release

- The first `0.0.x` release of `rim-server` and `rim-ui` through the release checklist
  (`docs/operations/release-checklist.md`): signed server images and desktop builds for Linux
  and Windows.
- The public relay `relay.rim-platform.org` opens as a first test run, hosting the RIM community
  workspace every new account joins, with the web client on `chat.rim-platform.org`
  (`docs/operations/relay-public.md`).
- Signed releases of the four first-party plugins, so "Install from link" works for each.

## Next, after the first release

In order:

1. **Bots and incoming webhooks.** A bot account a workspace Owner or Moderator creates, with
   tokens it can revoke; one incoming webhook URL per channel, bound to a bot, with its own rate
   limit; a GitHub preset first, so releases post themselves. Bots never join encrypted
   conversations.
2. **RIM in Claude Code and other agents.** The desktop app serves RIM as a local MCP server,
   and a Claude Code plugin packages it with a skill, so an agent on your machine can read your
   conversations, search, draft replies and run commands. It acts as you, through your desktop
   app: your keys stay on the device, encrypted conversations work, and every send or change
   waits for your confirmation in RIM, under the same command registry and gate as the built-in
   agent. Read-only by default; an encrypted conversation reaches the agent's model only with a
   grant for that conversation, and the audit log records what the agent did.
3. **The outside cryptographic review**, then `0.1.0`. Before it: a "why can't I read this" view
   that explains an undecryptable message, and a manual multi-device test pass in the release
   checklist.
4. **A first-run path** in the clients that grows with the product: create a key and write down
   the phrase, choose a drive for the archive, pick a relay, install a first plugin.
5. **Data export** per person and per workspace.
6. **Outgoing webhooks and richer pipelines**: posting, calling a webhook and setting state as
   pipeline actions, conditions, run history and retries; commands a bot registers in the
   palette.
7. **Member lists at relay scale**, so a workspace with thousands of members does not load them
   all at once, and handle lookup for `@username:rim-platform.org`.

## Later

Not scheduled; picked up when the items above are done or when someone needs them enough to
build them.

- **Calls:** voice calls with the team, and video and screen sharing after them, because talking
  a thing through is sometimes better than writing it. They follow the same rules as messages:
  calls in DMs and private groups are end-to-end encrypted, and a self-hosted server can carry
  them without a third-party service. Whether the media runs through the server or a separate
  self-hosted media server is decided when the work starts.
- **The agent:** local and self-hosted models (Ollama and OpenAI-compatible endpoints); macros
  the agent runs step by step with a pause at each change; catch-me-up summaries with citations,
  acting through commands rather than as a separate chatbot.
- **More MCP:** MCP servers installed as plugins, so the built-in agent can use them; and a
  headless MCP server with a bot token, for agents that run without a person at a desktop.
- **`rim-cli`:** `rim send`, `rim search`, `rim watch` and `rim macro run` with a host-local
  token; it doubles as a contract test of the GraphQL API.
- **Reach:** the web app as an installable PWA with web push and a narrow layout. No native
  mobile app; the cache and encryption code is Rust a future mobile build can reuse.
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

## Proposing a change to the roadmap

Open an issue in `rim-server` with the feature request template. It asks what you want, for
whom, and whether it belongs in the core or in a plugin; `CONTRIBUTING.md` ("What belongs in the
core") says where that line is.
