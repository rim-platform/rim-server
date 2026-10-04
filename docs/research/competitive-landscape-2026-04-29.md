# Competitive Landscape & "What To Build Next" — Strategic Memo

**Date:** 2026-04-29
**For:** Jakub (CEO)
**From:** Doctor Strange (strategic foresight, RIM)
**Status:** Strategy input. No code, no spec.

---

## TL;DR

Top recommendation in five bullets. If you read nothing else, read this:

- **Build the MCP host/server bridge next** (P4 in roadmap; "RIM is accidentally MCP-shaped" per Falcon). Slack just shipped Slackbot-as-MCP-client in Q1 2026 and made MCP table stakes for any "agentic workspace." RIM already has the capability gate, audit log, command registry, and confirmation flow. Wrapping commands as MCP tools and accepting MCP servers as a plugin runtime kind is a 2-3 week reach for a defensible "every team chat is now AI-shaped, RIM is the one with real consent" story.
- **Don't pivot to voice/video, Slack-style AI agents, or federation.** All three are commodity work that dilute RIM's keyboard/programmability story without earning new positioning.
- **Ship the CLI v1 in parallel** (P3, no competing chat product has one). Cheapest unlock, validates the GraphQL contract, and feeds keyboard/scriptability narrative directly. Days, not weeks.
- **The plugin marketplace is real but cold-start risk is high.** Mattermost (~70 marketplace plugins after years), Obsidian (2700+) shows what success looks like — and it took Obsidian ~3 years and a dedicated PKM community. Don't treat marketplace as a 2026 launch milestone; treat it as an 18-month outcome.
- **One stated differentiator is weaker than it sounds: "shared codebase across desktop and web."** It's a development-velocity win for RIM; users don't buy chat products on this. Reframe it internally as "we ship features once" rather than as a customer-facing pitch.

---

## 1. Question

> "What should we build next?" Registration redesign is cleared. Phase 5 (AI agent) is mid-flight but the unblocked design surface is open. Provide a strategic input memo: competitor landscape, gaps RIM can credibly attack, and 3-5 ranked feature proposals.

---

## 2. Competitive findings

I picked five competitors that actually move our positioning, plus two prior-art references for the plugin story. Skipped ones (Zulip, Revolt, Rocket.Chat) appear in `docs/competitive-analysis.md` already and don't change the picture.

### Slack (the incumbent — biggest threat just shifted under us)

- **Workflow Builder** is now the primary programmability surface: 65+ third-party connectors, AI-built workflows from natural language, ~3M workflows/day, 80% of builders are non-technical. ([Slack Workflow Builder](https://slack.com/blog/news/workflow-builder-connectors))
- **Slackbot reframed as agentic OS** (Q1 2026): 30 new AI features, **Slackbot is now an MCP client** that routes work to Agentforce / external agents. GA for Business+/Enterprise+ on 2026-01-13, Free/Pro rolling out April 2026. ([TechCrunch — Salesforce announces AI-heavy makeover for Slack](https://techcrunch.com/2026/03/31/salesforce-announces-an-ai-heavy-makeover-for-slack-with-30-new-features/), [Slack AI Agents](https://slack.com/ai-agents))
- **Custom keybinds remain unsupported.** Per Slack's help docs, you can adjust ~3 settings (Enter behaviour, etc.) and that's it. "Slack does plan to add this feature in the future" but no shipped date. ([Slack keyboard shortcuts](https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts))
- **No client-side plugin model.** Apps are server-side only.
- **What this means for RIM:** Slack's AI play is the threat — they're racing to be "the chat layer of the agentic enterprise." But their lock-in is also the trap: 80% non-technical workflow builders means they can't go deep on power-user programmability without alienating the base. The keybind/macro/palette gap stays open.

### Discord

- **Client extensibility is a thriving black market.** Vencord (62k Discord server members), BetterDiscord, Equicord (300+ plugins), Replugged. All ToS-violating but Discord enforces inconsistently. ([Vencord FAQ](https://vencord.dev/faq/), [BetterDiscord alternatives 2026](https://www.how2shout.com/alternatives/best-betterdiscord-alternative/))
- **Discord has chosen not to ship a first-party client plugin system in 7+ years.** They've explicitly considered it ([proposal thread](https://support.discord.com/hc/en-us/community/posts/360056064111-A-proposal-to-solve-BetterDiscord-and-other-client-mods)) and declined.
- **What this means for RIM:** Demand for client-side extensibility on a chat product is proven (62k people will install untrusted code on a daily-driver app). But Discord's audience is gaming/community — the demand pattern in *team* chat is unproven. The signal is real, the addressable market is the question.

### Mattermost (closest plugin-model parallel)

- **Go (server) + TS/React (client) plugin model with in-app marketplace.** ([Mattermost Plugins docs](https://docs.mattermost.com/integrations-guide/plugins.html), [Web app plugins](https://developers.mattermost.com/integrate/plugins/components/webapp/))
- React components register into channel header, sidebars, user popover, main menu. Closest architectural prior art to RIM's UI slot model.
- **No command palette, no custom keybinds, no macros.** Plugin marketplace exists but plugin count is modest (~70 listed; their power apps like Calls, GitHub, Jira are first-party).
- **What this means for RIM:** Mattermost validates that a chat-product plugin marketplace can exist, and that capability-gated UI extension points are the right shape. It also illustrates the cold-start problem — 8 years in and they still rely heavily on first-party plugins.

### Element / Matrix

- **Element X (mobile) is now the modern client; desktop repo was archived 2026-03-25** in favor of Element X-flavored web/desktop. ([element-desktop GitHub](https://github.com/element-hq/element-desktop), [Element X on Matrix.org](https://matrix.org/ecosystem/clients/element-x/))
- Extensibility = widgets (iframe embeds) + bridges + bots. **No command palette, no custom keybinds.**
- E2EE is best-in-class (Olm/Megolm, audited).
- **What this means for RIM:** Federation/E2EE is Element's moat, not RIM's. Don't chase it short-term — it's a 6+ month dedicated phase per `actual_state.md`.

### Linear (keyboard-first reference)

- **Cmd+K command palette** is a daily-driver feature; users do everything from it (create issues, assign, filter, navigate). ([Linear keyboard shortcuts](https://shortcuts.design/tools/toolspage-linear/), [Linear contextual command menu changelog](https://linear.app/changelog/2019-10-07-contextual-command-menu))
- Linear is project management, not chat — but it is the reference standard for "keyboard-first SaaS that users love." Their G-prefixed shortcuts and contextual command menu inform RIM's existing design.
- **What this means for RIM:** Validates that command-palette-as-primary-UI is a winning pattern in modern SaaS. RIM already has this shipped.

### Beeper (multi-protocol play)

- Now Automattic-owned, merged with Texts.com. Supports Android SMS/RCS, WhatsApp, Telegram, Signal, Messenger, Instagram, Twitter, Slack, Discord, LinkedIn, Google Chat, IRC, Matrix. ([Beeper](https://www.beeper.com/), [TechCrunch — Beeper relaunch](https://techcrunch.com/2025/07/16/beepers-all-in-one-messaging-app-relaunches-with-an-on-device-model-and-premium-upgrades/))
- $9.99/mo (10 networks), $49.99/mo (unlimited).
- **What this means for RIM:** "One client, many protocols" is a real consumer play but Beeper owns it. RIM going there means competing with $125M of Automattic capital on a thesis (consumer multi-protocol) that isn't team-chat. Skip.

### Prior art (not competitors, but informative)

- **VS Code marketplace:** 50,000+ extensions in 2026. ([VS Code Marketplace](https://marketplace.visualstudio.com/vscode)) The reference success.
- **Obsidian:** 2700+ community plugins. Took ~3 years to reach critical mass. ([Obsidian Plugins](https://obsidian.md/plugins))
- **Zed channels:** Editor-native chat with voice + shared notes + project context. ([Zed Channels](https://zed.dev/docs/collaboration/channels)) Adjacent — Zed is going where some of RIM's users live (technical teams who want communication near code). Not a chat replacement, but worth tracking.

### MCP — the new entrant that changes the rules

- Anthropic's Model Context Protocol shipped Nov 2024. By March 2026: **10,000+ active public MCP servers, 97M monthly SDK downloads**. ([Workos MCP guide 2026](https://workos.com/blog/everything-your-team-needs-to-know-about-mcp-in-2026))
- All major AI clients support it: Claude, ChatGPT, Gemini, Copilot, Cursor, Windsurf, VS Code, Zed.
- Slack now an MCP client (above). Linear, Notion, Sentry, Figma all have MCP servers.
- **What this means for RIM:** MCP is becoming the "USB-C of AI integrations." A chat product without MCP fluency in 2026 is positioning itself as legacy.

---

## 3. Gap analysis — where RIM's strengths actually create defensible value

### Stated differentiators, ranked by defensibility

| Differentiator | Defensibility | Notes |
|---|---|---|
| Command palette + macros + custom keybinds (the trifecta) | **Strong** | Verified via direct competitor docs: none of Slack, Discord, Mattermost, Zulip, Element offer this combination. Linear validates the pattern in adjacent SaaS. |
| Client-side plugin system with capability gates | **Medium-strong** | Mattermost is the only direct parallel. The capability-gated model is more rigorous than anyone else's. Cold-start ecosystem risk is real (see "What I'm uncertain about"). |
| Capability-gated AI agent | **Strong, time-limited** | RIM's confirmation-proof + audit + capability check is genuinely better-engineered than what Slack shipped. Window is 12-18 months before Slack's enterprise team builds equivalent. |
| Tauri v2 (non-Electron) runtime | **Weak as a customer pitch** | Real engineering win (memory, binary size). Users don't buy on this. Use as a recruiting / dev-velocity story, not a product pitch. |
| Future E2EE | **Defer** | Element owns this category and is years ahead. A 6+ month phase with low user pull. Skip unless a privacy-segment customer specifically asks. |
| **Shared codebase desktop+web** | **Internal advantage, not a customer-facing differentiator** | This is "we ship features once." Users don't compare products on this. Frame it as engineering leverage, not as positioning. |

### What RIM has that Slack/Discord/Mattermost don't, today

- A real command palette, shipped, with ~30 commands ([actual_state.md §2](../actual_state.md))
- Chainable macros with pre-validation pass and per-stage results — verified shipped
- 19 default keybinds, fully user-customizable per workspace, context-aware
- COMPOSE/NAVIGATE vim-mode anchored to composer focus (j/k nav, r/e/t/p actions) — unique
- Capability-gated agent with audit log, sensitivity tiers, confirmation proofs, single-use replay block

### What RIM is missing that competitors have

- **Client-side plugin breadth.** Plugin host runs but discovery is tiny; only RIM-built plugins exist. Mattermost has ~70 marketplace entries; Slack has 2,600+ apps.
- **MCP fluency.** Zero today. Slack shipped this in Q1 2026. This is the closing window.
- **Voice/video.** Discord owns this; not a gap RIM should fill.
- **Federation.** Element owns this; not a gap RIM should fill.
- **CLI / scriptable client.** Nobody in chat has this. Not a "missing" — an opportunity.

### Table-stakes check (what's already shipped)

Per `docs/actual_state.md` (Tony Stark, 2026-04-25): threading, file uploads, mentions, reactions, search, pinning, forwarding, typing indicators, read state, pagination, link previews, custom emoji, permissions — all green. **Table stakes are not the bottleneck.**

What's broken or partial right now:
- Plugin AI consent dispatch (#5, Pete remediating)
- Frontend↔Rust `PluginInfo` schema drift (Pete remediating)
- `synthesize_command_definitions` (Phase 5 #4) — agent can't discover macros
- `command.discover` ship gate (#10) blocked behind #2/#4/#5/#9
- Backend `plugin_host` module is 1,290 LOC of dead code (Phase 6 cleanup)

---

## 4. Feature proposals — ranked by impact-to-effort

### Rank 1: MCP host/server bridge

**Pitch.** RIM both speaks MCP (RIM commands appear as tools to external MCP clients) and consumes MCP (MCP servers appear as plugins inside RIM). One protocol, two directions.

- **Differentiator leveraged.** Plugin system + capability-gated AI. RIM's confirmation/audit/sensitivity model maps cleanly onto MCP's tool invocation contract. Plugin host already mediates external processes.
- **Target user.** Technical teams running internal MCP servers (10,000+ public servers exist, far more private). Anyone whose ops/dev workflows already pass through Cursor/Claude/Zed.
- **Effort.** **Medium (2-3 weeks).** Falcon's "RIM is accidentally MCP-shaped" claim is correct on inspection: capability gates, command DTOs with `nl_description`/`intent_samples`/`sensitivity`, audit log, confirmation proofs all map. Net new work: protocol shim, MCP server discovery in plugin manifest, MCP client-mode for the agent.
- **Risks / dependencies.**
  - Depends on Phase 5 #5 (plugin AI consent) landing cleanly. Schema drift fix is the prerequisite.
  - Spec churn — MCP is still maturing. Lock to a versioned spec snapshot.
  - Confirmation UX needs to handle "MCP server invokes RIM command" flows where the initiator is neither user nor first-party agent. The `CommandInitiator` enum (already shipped per `actual_state.md`) covers this.
- **What competitors do.** Slack (Slackbot as MCP client, GA Q1 2026). Linear, Notion, Sentry, Figma all expose MCP servers. **No team-chat product is yet an MCP host that lets external MCP servers become first-class extensions in the chat workspace.** That's the gap.
- **Why this ranks #1.** It converts an existing investment (Phase 5 capability work) into market-leading positioning at moderate cost. Slack's MCP-client move means the "AI workspace" framing is now table stakes — but RIM can leapfrog by being bidirectional.

### Rank 2: CLI v1 (`rim` binary)

**Pitch.** Scriptable command-line client. `rim send`, `rim search`, `rim macro run`, `rim watch`. Phase 1 of the roadmap-defined CLI track.

- **Differentiator leveraged.** Keyboard-first / scriptability story directly. Reinforces "RIM adapts to how you work" — for the user who lives in tmux, this is a day-one love-it feature.
- **Target user.** DevOps, SREs, developers who incident-respond from terminal, anyone doing automation on top of chat.
- **Effort.** **Small-to-medium (3-7 days for v1).** Design already exists at `rim-server/docs/design/cli-v1-design-2026-04-22.md`. GraphQL contract is stable. Net work is a Rust binary + auth (reuse session cookie or PAT), not new backend.
- **Risks / dependencies.**
  - PAT issuance flow if we don't want to ask for password on the CLI. Likely needs a small backend addition.
  - Keep scope tight to Phase 1 of the existing design (no TUI yet).
- **What competitors do.** Slack has `slack-cli` for *workflow/app development*, not as an end-user chat client ([Slack CLI](https://docs.slack.dev/tools/slack-cli/)). Discord, Mattermost, Element: nothing comparable. **Genuine gap.**
- **Why this ranks #2.** Cheapest unlock that produces a defensible "look how programmable RIM is" demo. Validates the GraphQL contract end-to-end. Feeds the differentiator narrative without disturbing in-flight Phase 5 work. Could ship in parallel.

### Rank 3: Plugin SDK + 3 first-party reference plugins

**Pitch.** Publish a thin plugin SDK (`rimkit-python`, `rimkit-typescript`) and ship 3 reference plugins that exercise the full extension surface: a tool_panel UI plugin, a headless event-driven plugin, and a context-action plugin. Treat as a launch-readiness move for the marketplace, not the marketplace itself.

- **Differentiator leveraged.** Plugin system with marketplace (the #1 stated differentiator).
- **Target user.** Developers willing to build plugins. The first 50 plugin authors will define what RIM's ecosystem becomes.
- **Effort.** **Medium-Large (3-5 weeks).** SDKs themselves are ~1 week each (the protocol is JSON-RPC over stdio per `docs/local_plugin_host.md` — small surface). Reference plugins: 3-5 days each. Documentation pass: 1 week.
- **Risks / dependencies.**
  - Dead-code cleanup of backend `plugin_host` module (1,290 LOC) should happen first or alongside, otherwise public docs reference modules that aren't wired.
  - The `injectPluginTools` schema drift must be fixed before docs go live or first-party plugins demonstrate broken AI integration.
  - **The cold-start trap.** Mattermost has ~70 marketplace entries after 8 years. Obsidian has 2700+ after ~5 years but won the PKM crowd organically. Plugin marketplaces are a 2-3-year game, not a 6-month outcome. Set internal expectations accordingly.
- **What competitors do.** Mattermost has the closest model. Slack/Discord have server-side app directories (different shape). Vencord/BetterDiscord show that *demand* exists for client-side mods on chat. ([Vencord](https://vencord.dev/))
- **Why this ranks #3.** It's the right strategic investment but will not show ROI inside Q3. Rank 1 and Rank 2 produce visible differentiation faster. This one builds the foundation.

### Rank 4: Skill Macros launch (finish Phase 5 #4 properly)

**Pitch.** Land Skill Macros to completion (the 6 BLOCKs Hawkeye returned), then ship the user-facing UX — agent suggests macros from natural language, user approves once, macros become discoverable and replayable.

- **Differentiator leveraged.** Macros + AI-agent + capability-gated execution. This is the differentiator stack RIM has been building toward.
- **Target user.** Power users who already use macros; agent users who currently can't discover them (`actual_state.md` confirms macro discovery is broken).
- **Effort.** **Medium (2-3 weeks of focused work)** per Tony's Category A estimate (3-5 days to honestly close Phase 5, then UX layer).
- **Risks / dependencies.**
  - Currently in active rework (Strange remediating 6 BLOCKs). Don't add scope until that's clean.
  - The actual product closure (`synthesize_command_definitions` + `command.discover`) is a ship gate, not a feature. Treat it as "finish what's started," not "new bet."
- **What competitors do.** Slack Workflow Builder is the analog but it's GUI-driven and non-technical. RIM macros are command-composable and far more powerful. **Different audience, real differentiation.**
- **Why this ranks #4.** It's higher value than Rank 5 but lower-priority than #1-3 because it's catch-up work on an in-flight phase rather than a fresh strategic bet. Tony's Category A guidance ("finish Phase 5 honestly") supports doing this *before* picking Rank 1.

### Rank 5: Public landing-page positioning + first 5 design-partner customers

**Pitch.** This isn't a feature — it's a process recommendation. RIM's product story is unusual enough that the next 5 paid users are worth more than the next 50 features. Find them, watch them, build the next phase from their friction.

- **Differentiator leveraged.** All of them — used as positioning hooks rather than as code.
- **Target user.** Technical teams of 5-50 with internal tooling needs. Likely DevOps-heavy, security-conscious, or research-coded.
- **Effort.** **Small in code (negligible). Large in attention.** This is a Jakub/Fury input call.
- **Risks / dependencies.** Premature scaling without product-market signal is the main risk. Five real users beat 500 waitlist signups for steering.
- **Why this ranks #5.** Listed for completeness — it's the input loop that should drive ranks 1-4 over the next 2 quarters. Not a build proposal per se.

---

## 5. What NOT to build

Three candidates that look attractive and would dilute focus:

### Voice / video calling
Discord-grade voice is months of infrastructure work (WebRTC SFU, TURN servers, NAT traversal, codec pipelines). Multiple competitors do this well. Already deferred indefinitely per `actual_state.md`. **Stay deferred.** If a customer demands voice, point them at Jitsi-as-plugin (the existing fallback plan) and revisit only if the demand pattern is repeatable across 3+ design partners.

### Slack-style "AI agent with 30 features" expansion
Tempting — Slack just did it, the press is loud. But Slack's agent is an enterprise sales play backed by Salesforce CRM. RIM's agent is a technical-team productivity play backed by capability gates. Trying to match Slack's breadth (HR, IT service, Sales) drags RIM into commodity territory and dilutes the consent/audit moat. **Stay narrow:** the agent is a tool-caller for commands and macros, not a horizontal AI assistant.

### Federation / Matrix interoperability
Element owns this. Building a Matrix bridge would consume 6+ months for a feature that the audience RIM targets (programmability-first technical teams) doesn't strongly want. Different segment, different product. **Skip unless a specific customer pays for it.**

### "Chat with every protocol" Beeper-style multiplexing
Automattic dropped $175M ($125M Beeper + $50M Texts.com) on this thesis. RIM is not equipped to win that race and the audience is consumer, not team. **Skip.**

---

## 6. What I'm uncertain about

Honest list of claims I couldn't fully nail down or where I'm reasoning from limited signal:

- **Mattermost marketplace size.** I asserted "~70" — that's a rough order-of-magnitude from search results, not a verified count. The marketplace exists and is small relative to VS Code (50k) or Obsidian (2700+); the precise number doesn't change the conclusion.
- **MCP adoption velocity in team-chat specifically.** I'm extrapolating from Slack's Q1 move + Linear/Notion having MCP servers. Whether *teams* (vs individual developers) actually want MCP-in-chat is unproven. RIM building this is a bet, not a confirmed market pull.
- **Slack custom keybinding roadmap.** Sources say Slack "plans to add custom keybinds" but no shipped date or commit. If they ship in 2026, RIM's keybind moat narrows. Worth a check-in every 90 days.
- **Element X desktop status.** The element-desktop GitHub repo was archived 2026-03-25 with a redirect to Element X-flavored web. I'm not 100% sure whether desktop is now a thin Element X wrapper or a separate codebase — sources are ambiguous. Doesn't change RIM's positioning either way.
- **Cold-start economics for RIM's marketplace.** I argued it's a 18-month outcome. That assumes RIM follows Mattermost/Obsidian curves. If the audience is different (more like VS Code's developer audience, less like Slack's enterprise audience), the curve could be steeper *or* shallower. Genuine unknown.
- **Whether Falcon's "RIM is accidentally MCP-shaped" claim survives detailed inspection.** I read the capability_security_model + plugin_model + the AI reframe design docs and the shape looks right, but a real spike from `iron-man` would prove or disprove the 2-3 week estimate. Treat that estimate as "S/M/L → M" rather than as a committed timeline.
- **Overlap with parallel agents.** The user noted vision/iron-man may be investigating plugin discovery. I haven't read their in-flight findings. If their work materially changes the readiness of Rank 3, this memo's effort estimate for the SDK push could be optimistic.

---

## 7. Recommended sequencing for Jakub

If you accept the ranking:

1. **This week:** Tony's Category A — finish Phase 5 honestly. Rank 4 lands as part of that.
2. **Next 2 weeks (parallel track, low coordination cost):** Rank 2 (CLI v1). Cheap, validates the GraphQL contract, ships keyboard-first narrative.
3. **Phase 6 (post Phase 5 close):** Rank 1 (MCP bridge). The strategic bet. Time-sensitive against Slack's positioning.
4. **Q3:** Rank 3 (plugin SDK + reference plugins). Foundation for the marketplace play. Don't expect ecosystem ROI inside 2026.
5. **Throughout:** Rank 5 (5 design-partner customers). The signal that should redirect 1-4 if they're wrong.

This is opinion. You decide; Fury delegates.

---

## Sources

- [Slack Workflow Builder — connectors expansion](https://slack.com/blog/news/workflow-builder-connectors)
- [Slack Workflow Builder docs](https://docs.slack.dev/workflows/workflow-builder/)
- [Slack keyboard shortcuts help article](https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts)
- [Slack AI Agents page](https://slack.com/ai-agents)
- [TechCrunch — Salesforce announces AI-heavy makeover for Slack with 30 new features](https://techcrunch.com/2026/03/31/salesforce-announces-an-ai-heavy-makeover-for-slack-with-30-new-features/)
- [Slack Becomes an Agentic OS — MCP integration](https://aiautomationglobal.com/blog/slack-ai-agentic-os-mcp-30-features-2026)
- [Mattermost Plugins documentation](https://docs.mattermost.com/integrations-guide/plugins.html)
- [Mattermost Web app plugins](https://developers.mattermost.com/integrate/plugins/components/webapp/)
- [Mattermost Marketplace](https://mattermost.com/marketplace/)
- [Linear keyboard shortcuts reference](https://shortcuts.design/tools/toolspage-linear/)
- [Linear contextual command menu changelog](https://linear.app/changelog/2019-10-07-contextual-command-menu)
- [Beeper relaunch coverage — TechCrunch](https://techcrunch.com/2025/07/16/beepers-all-in-one-messaging-app-relaunches-with-an-on-device-model-and-premium-upgrades/)
- [Beeper homepage](https://www.beeper.com/)
- [Vencord — Discord client modification](https://vencord.dev/)
- [Vencord FAQ](https://vencord.dev/faq/)
- [BetterDiscord alternatives 2026 comparison](https://www.how2shout.com/alternatives/best-betterdiscord-alternative/)
- [element-desktop GitHub (archived 2026-03-25)](https://github.com/element-hq/element-desktop)
- [Matrix.org — Element X](https://matrix.org/ecosystem/clients/element-x/)
- [Obsidian community plugins](https://obsidian.md/plugins)
- [VS Code Marketplace](https://marketplace.visualstudio.com/vscode)
- [Zed channels documentation](https://zed.dev/docs/collaboration/channels)
- [Zed introducing channels blog post](https://zed.dev/blog/channels)
- [WorkOS — Everything your team needs to know about MCP in 2026](https://workos.com/blog/everything-your-team-needs-to-know-about-mcp-in-2026)
- [Discord proposal thread on solving BetterDiscord](https://support.discord.com/hc/en-us/community/posts/360056064111-A-proposal-to-solve-BetterDiscord-and-other-client-mods)
- [Notion vs Coda 2026 — Automation Atlas](https://automationatlas.io/guides/notion-vs-coda-comparison-2026/)

### Cross-references inside RIM repos

- `/home/kuba/RustroverProjects/rim-server/docs/vision.md`
- `/home/kuba/RustroverProjects/rim-server/docs/competitive-analysis.md`
- `/home/kuba/RustroverProjects/rim-server/docs/actual_state.md`
- `/home/kuba/RustroverProjects/rim-server/docs/plugin_model.md`
- `/home/kuba/RustroverProjects/rim-server/docs/keybind_model.md`
- `/home/kuba/RustroverProjects/rim-server/docs/design/cli-v1-design-2026-04-22.md`
- `/home/kuba/RustroverProjects/rim-server/docs/design/mcp-bridge-sketch-2026-04-22.md`
