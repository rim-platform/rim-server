# RIM ⇄ MCP Bridge — Architecture Sketch

**Author:** Falcon (Sam Wilson, recon)
**Date:** 2026-04-22
**Audience:** Kuba (decision: build/defer/kill), Tony (CTO sequencing), Nick Fury (architecture review when promoted to design), Black Widow (security touchpoints flagged)
**Status:** **Sketch — not a design.** Goal is to give Kuba enough shape and effort context to decide whether to commit. A real design comes after Kuba says "yes, build it."

> "Cap, get out of there!" — Sam, on premature commitment to a moving spec
> MCP became infrastructure in 2026. Not implementing this is a strategic gap. *How* and *when* we implement is the call.

---

## 0. Why This Matters in One Paragraph

MCP (Model Context Protocol) shipped as an Anthropic open standard in late 2024, and through 2025–early 2026 became the de-facto plug-protocol for AI tooling: Claude Desktop, Cursor, Goose, ChatGPT, VS Code, and others all speak it. The newer **MCP Apps** extension (officially shipped Jan 2026 — see [blog.modelcontextprotocol.io/posts/2026-01-26-mcp-apps](https://blog.modelcontextprotocol.io/posts/2026-01-26-mcp-apps/)) lets MCP servers ship sandboxed-iframe UI alongside their tools — *which is structurally identical to RIM's existing plugin slot model.* RIM already has the right shape; we'd be wiring it up, not inventing it. The strategic question is whether to be a first-class MCP host *and* server now (catches the platform wave) or to wait and be a follower.

---

## 1. Two Halves (Both Required for Strategic Effect)

| | **RIM as MCP Host** | **RIM as MCP Server** |
|---|---|---|
| Direction | RIM → external tools | External AI clients → RIM |
| Examples | RIM uses `filesystem`, `github`, `youtrack` MCP servers | Claude Desktop drives RIM: "post message to #ops" |
| User benefit | Plugin marketplace explodes overnight (any MCP server installs) | RIM is reachable from any AI workspace, not just its own |
| Required first | Plugin slot model (have it) + command registry (have it) | API token model (CLI v1, Task #12) + MCP server protocol layer |
| Strategic value | Solves "who builds RIM plugins?" — the MCP ecosystem already did | Makes RIM the *target* of other workspaces' AI, not their competitor |

**Both matter. Skipping the server half means RIM is a consumer of the MCP ecosystem; shipping both means RIM is a peer.** Kuba's call.

---

## 2. MCP Host Architecture (RIM consumes external MCP servers)

### 2.1 Crate placement

New crate: **`rim_mcp_host/`** in workspace, sits at the same layer as `rim_infra_services` (it's an external-facing service implementation). Depends on `rim_services` (provides `CommandService` to register external tools as commands), `rim_core` (ID types), and an MCP client SDK (`rmcp` looks like the leading Rust option as of 2026-04 — needs verification).

**Why a separate crate:** subprocess management, JSON-RPC framing, stdio transport are lifecycle-heavy concerns that don't belong in `rim_infra_services`. Isolating them means the rest of the backend doesn't know MCP exists — they just see new commands appear in the registry.

### 2.2 Plugin model alignment

**Critical insight from the AI Agent audit (Task #6, Part B.4):** RIM's plugin model is already JSON-RPC 2.0 over stdin/stdout with capability-based permissions (`docs/plugin_model.md`). MCP's stdio transport is *literally the same shape* — JSON-RPC 2.0, line-delimited, over a subprocess's stdin/stdout (per [modelcontextprotocol.io/specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)).

**This means an MCP server is just a special kind of RIM plugin.** The plugin manifest gains a new `kind: "mcp"` variant; the local plugin host knows how to speak the MCP handshake (`initialize`, `tools/list`, `tools/call`) instead of RIM's native plugin RPC. Everything downstream — capability prompts, sandboxing, install/enable/disable — reuses Phase 4 plumbing.

### 2.3 Tool discovery → command registration

On plugin enable:
1. Local plugin host spawns the MCP subprocess (per existing Phase 4 lifecycle).
2. Sends `initialize` per MCP handshake.
3. Sends `tools/list` → receives `[{ name, description, inputSchema }, ...]`.
4. **For each tool, registers a command** in the unified `CommandRegistry` (per Task #6 audit Part B):
   - `id`: `mcp.<server-name>.<tool-name>` (e.g., `mcp.filesystem.read_file`)
   - `title`: from MCP `description` (first line)
   - `description`: from MCP `description` (full)
   - `nl_description` and `intent_samples`: empty for v1; future MCP spec extension or LLM-generated at install time
   - `arguments`: derived from MCP `inputSchema` (JSON Schema → `CommandArgumentDto[]` mapping). MCP schemas are JSON Schema → trivial conversion.
   - `source_kind`: new `MCP` variant on `CommandSourceKindDto`
   - `execution_target`: `MCP_RPC` (new variant)
   - `dangerous`: defaults to `true` for any tool the server marks as having side effects (or all of them, conservatively, in v1 — let user override)
   - `required_capabilities`: `["mcp.<server-name>"]` — single capability per MCP server, capability prompt at install time

5. On `tools/list/changed` notification (MCP supports this), re-register.

**Result:** an MCP server's tools become first-class commands. They show up in the palette. They're keybind-able. The agent (post-reframe) calls them through the same `CommandService::execute_command` path. **No parallel tool system.** This is the architectural payoff.

### 2.4 Execution path

`CommandService::execute_command("mcp.filesystem.read_file", args)`:
1. Existing dispatcher (`builtin_executor.rs:67-108` — see audit Part B.7) sees `execution_target = MCP_RPC`.
2. Routes to the MCP host runtime.
3. MCP host serializes `{ method: "tools/call", params: { name: "read_file", arguments: <args> } }` over the subprocess stdin.
4. Reads response from stdout.
5. Wraps in `CommandResult` and returns. Same result shape as built-ins. Same error mapping.

**Audit trail:** unified. The `CommandService::execute_command` log line records who, what, when — same for built-ins, plugins, MCP tools.

### 2.5 Capability and sandbox model [G-MCP-1, G-MCP-2 — Black Widow]

**G-MCP-1: capability granularity.** MCP doesn't have a standard capability/permission protocol. A server declares its tools; the host either trusts the whole server or doesn't. RIM's existing capability model (`docs/capability_security_model.md`) is finer-grained — per-tool prompts.

Two options:
- **Coarse (v1):** one capability per MCP server (`mcp.filesystem`). User approves the server, all its tools are usable. Matches Claude Desktop's model.
- **Fine (v1.5):** RIM intercepts each `tools/call`, prompts on first use of each tool. More secure, more friction. Optional opt-in via plugin manifest.

Recommend: coarse for v1, fine as an optional manifest flag. Enterprise users can require fine.

**G-MCP-2: subprocess sandbox.** MCP servers are arbitrary subprocesses. Per spec they're trusted local code (the user installed them). But a malicious filesystem MCP server can read anything the user can. Mitigations:
- Run subprocesses with reduced privileges where OS allows (Linux: bubblewrap/landlock; macOS: sandbox-exec; Windows: AppContainer). Significant work — defer to v1.5.
- Filesystem allowlist via plugin manifest — user-declared, enforced via OS sandbox. Defer.
- v1: run as user, document the trust assumption clearly. **Same posture as `npm install` — community-installed code runs with your privileges.**

**For Black Widow:** the v1 sandbox posture is "trust the user's install decision." Need explicit signoff, especially given that AI agents will be invoking these tools without per-call prompts (under coarse capability model). Suggest documenting "Plugin Trust Statement" in the install prompt UX.

### 2.6 The "MCP Apps" payoff (Jan 2026 spec)

[MCP Apps](https://blog.modelcontextprotocol.io/posts/2026-01-26-mcp-apps/) lets a tool return interactive UI (sandboxed iframe with HTML/JS, communicating with host via `postMessage`). Tools include `_meta.ui.resourceUri` pointing at a `ui://...` resource the host fetches and renders.

**RIM's plugin slot model already does this.** Phase 4 plugins ship sandboxed iframes that communicate with the host via `postMessage` and have capability-scoped APIs (per `docs/plugin_model.md`). The mapping is direct:

| MCP Apps concept | RIM equivalent | Adapter needed? |
|---|---|---|
| Sandboxed iframe | RIM plugin panel iframe | None — same primitive |
| `postMessage` JSON-RPC | RIM plugin host bridge | Thin adapter (route MCP UI ops to RIM plugin host) |
| `ui://...` resource URI | Plugin asset path | Resolver — fetch from MCP server, cache locally, serve from plugin host |
| Tool returns UI ref | Command result with `panel_ref` | Add `panel_ref` to `CommandResultDto` |

**Implication:** when a hosted MCP server returns a UI alongside its tool result, RIM displays it in a plugin panel slot. *This is RIM's "AI panels" feature, for free, by virtue of being a good MCP host.*

This is the strongest argument for shipping the host half — the MCP ecosystem is doing the UI work.

---

## 3. MCP Server Architecture (RIM exposes itself to external clients)

### 3.1 Crate placement

New crate: **`rim_mcp_server/`** in workspace, parallel to `rim_mcp_host`. Depends on `rim_services` (to invoke commands), `rim_core` (ID types), MCP server SDK.

### 3.2 Transport: stdio + Streamable HTTP

Per [MCP transports spec](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports), two standard transports:

| | stdio | Streamable HTTP |
|---|---|---|
| Use case | Local clients (Claude Desktop on same machine) | Remote clients (Cursor connecting to RIM server in cloud) |
| Auth | None per spec (subprocess trust boundary) | OAuth 2.1 mandatory per spec |
| Implementation in RIM | New binary `rim-mcp` that launches per-client | New endpoint `POST /mcp` on existing rim-server |

**Note:** the older SSE transport is **deprecated** as of the 2025-06-18 spec. Don't implement it. Streamable HTTP is the remote story.

#### 3.2a stdio shape

User installs RIM, configures Claude Desktop with `claude_desktop_config.json`:
```json
{
  "mcpServers": {
    "rim": {
      "command": "/usr/local/bin/rim-mcp",
      "env": { "RIM_TOKEN": "rim_pat_...", "RIM_ENDPOINT": "https://chat.example.com/api" }
    }
  }
}
```

`rim-mcp` is a **thin transport bridge**. It speaks MCP on stdio, translates to GraphQL `executeCommand` mutations against the rim-server (using the same API token model from CLI v1, Task #12). **No new server-side code required for stdio** — just the bridge binary, which can live in `rim_mcp_server/` alongside the HTTP variant.

This is a key architectural decision: **`rim-mcp` is a CLI cousin, not a server module.** It reuses the entire CLI auth + transport stack.

#### 3.2b Streamable HTTP shape

Add `/mcp` endpoint to `rim_server/src/server.rs:81-98` (alongside `/api` for GraphQL). Auth: OAuth 2.1 per MCP spec — **major work** since RIM doesn't currently have an OAuth provider. Two paths:

- **Hard:** implement OAuth 2.1 server (auth code flow, PKCE, token endpoint, etc.). Significant — easily L effort by itself.
- **Pragmatic:** support API tokens via `Authorization: Bearer rim_pat_...` (same as CLI). **Spec-non-compliant for the OAuth requirement, but works in practice.** Document as "RIM-flavored MCP HTTP transport." Defer full OAuth to v2.

Recommend: pragmatic path for v1, OAuth in a later phase. Most remote MCP clients in 2026 still accept bearer tokens via custom config — strict OAuth-only enforcement isn't universal yet.

**For Black Widow:** flag — Streamable HTTP without OAuth is a deviation from spec. Document the deviation; ship with bearer support; track as tech debt.

### 3.3 Tool surface — what does RIM expose?

Three options:

| Option | What's exposed | Risk | Effort |
|---|---|---|---|
| **A. Everything** | All ~37 built-in commands + plugin commands + user macros | Foot-gun risk for `state.delete`, `chat.delete_message` etc. | Low |
| **B. Allowlist** | Only commands marked `mcp_safe: true` in `CommandDefinitionDto` | Conservative; some tools never reach external clients | Medium (need new DTO field + per-command audit) |
| **C. User-scoped allowlist** | Per API token, user picks which commands are MCP-callable in Settings | Most flexible, most UX work | High |

Recommend: **B for v1**, transition to **C in v1.5**. Add `mcp_safe: bool` to `CommandDefinitionDto` per the AI Agent audit's reframe (Task #6, Part E.2 — same shape as proposed `ai_callable`). In fact: **`ai_callable` and `mcp_safe` are likely the same field.** The agent and external MCP clients are both "AI invokers" at this layer.

### 3.4 Auth alignment with CLI v1

The MCP server (HTTP transport) and the CLI use the same API token model (Task #12, §2.2). `ApiToken` entity, `Authorization: Bearer rim_pat_…`, scope `full_user` in v1. **Zero new auth concepts.** This is the single biggest reason to do CLI v1 *first*.

---

## 4. MCP Apps Mapping in Detail

The Jan 2026 MCP Apps spec is the highest-leverage feature for RIM specifically because **the iframe-via-postMessage pattern is already RIM's plugin slot model.**

### 4.1 Direct mapping table

| MCP Apps concept | RIM concept | Mapping |
|---|---|---|
| Tool with `_meta.ui.resourceUri` | Command with `panel_ref` field on `CommandResultDto` (NEW) | When command result has a panel ref, frontend opens a plugin slot. |
| `ui://...` resource | Static asset bundle (HTML/JS) | RIM serves via `/api/plugins/<id>/assets/<path>` (existing). MCP host caches the resource locally on first access. |
| `postMessage` JSON-RPC bridge | Existing plugin host bridge | Adapter layer in `rim_mcp_host` translates between MCP UI RPC names and RIM plugin RPC names. |
| Iframe sandbox attributes | Existing plugin iframe sandbox | Reuse — RIM's already strict (no `allow-same-origin`, no `allow-top-navigation`). |
| App-to-host UI events | RIM plugin events | Same channel, different schema. |

### 4.2 Where the adapter sits

```
┌─────────────────────────────────────────────────┐
│ RIM Frontend (rim-ui)                            │
│  ┌─────────────────────────────────────────┐    │
│  │ Plugin Slot (sandboxed iframe)           │    │
│  │  [HTML/JS from MCP server's ui:// res]   │    │
│  └─────────────────────────────────────────┘    │
│         ↑ postMessage (bridge)                   │
│         ↓                                         │
│  [Plugin Host Bridge — adds MCP App adapter]    │
└─────────────────────────────────────────────────┘
         ↑
         ↓ existing GraphQL/WS
┌─────────────────────────────────────────────────┐
│ Backend (rim-server)                             │
│  rim_mcp_host                                    │
│  - subprocess per MCP server                     │
│  - on tools/call → returns result + ui meta      │
│  - frontend opens slot with ui:// resource       │
└─────────────────────────────────────────────────┘
```

**Net new code:** ~one adapter module on the frontend (~300 LOC, translates between the two RPC schemas) + the `panel_ref` plumbing on `CommandResultDto`. Everything else is reuse.

### 4.3 The killer combo

User has `youtrack` MCP server installed via RIM. Says to RIM agent: *"create a youtrack ticket for that bug."* Flow:

1. Agent matches intent → `mcp.youtrack.create_ticket` command (registered automatically per §2.3).
2. Agent fills args from context, dispatches via `CommandService::execute_command`.
3. MCP host calls `tools/call` on the youtrack subprocess.
4. Server returns: `{ result: { ticket_id, url }, _meta: { ui: { resourceUri: "ui://youtrack/ticket-detail" } } }`.
5. RIM frontend opens a plugin slot, fetches the UI resource, renders the ticket detail interactively (status dropdown, assignee picker, comment box).
6. User edits in-iframe → `postMessage` → MCP host → `tools/call` for updates.

**Result:** the agent did the speaking, MCP did the connecting, RIM did the rendering. Zero RIM-specific YouTrack code. *That's the platform play.*

---

## 5. Effort Estimate (Rough)

| Phase | Scope | Effort | Reasoning |
|---|---|---|---|
| **Pre-req** | CLI v1 (Task #12) ships | M | Establishes API token model — both MCP halves depend on it. |
| **Pre-req** | AI Agent reframe (Task #9) ships | S–M | Establishes unified Command Registry with `ai_callable`/`mcp_safe` field. |
| **Phase H1: MCP Host stdio** | `rim_mcp_host` crate, plugin manifest `kind: "mcp"`, tools/list → command registration, tools/call dispatch through `CommandService` | **L** | Subprocess lifecycle, JSON-RPC framing, schema translation, error handling. Plus plugin manifest extension and capability mapping. |
| **Phase H2: MCP Apps support (host side)** | Frontend adapter, `panel_ref` on CommandResultDto, ui:// resource fetch + cache | **M** | Mostly frontend. RIM's plugin slot infra does the heavy lifting. |
| **Phase S1: MCP Server (stdio bridge)** | `rim-mcp` thin binary that bridges stdio MCP → HTTP+bearer to rim-server | **M** | Reuses CLI auth/transport stack. Bidirectional JSON-RPC framing is the new bit. |
| **Phase S2: MCP Server (Streamable HTTP, bearer auth)** | `/mcp` endpoint on rim-server, message routing, mcp_safe filtering | **M** | New endpoint, but reuses CommandService. Skip OAuth → keep effort bounded. |
| **Phase S3: OAuth 2.1 for HTTP transport** | Spec-compliant OAuth provider | **L–XL** | Significant. Skip in v1; revisit if remote MCP clients start enforcing OAuth. |
| **Phase H3: Sandbox hardening** | OS-level subprocess isolation (bubblewrap/landlock/sandbox-exec) | **L** | Per-platform work. Defer to v1.5. |
| **Phase total v1** | H1 + H2 + S1 + S2 | **L–XL** combined | ~6–8 weeks calendar with one Rust dev + one frontend dev. |

**Cheapest viable shipment that captures both halves:** H1 + S1 only (~M+L = ~4-5 weeks, single dev). Drops MCP Apps UI rendering (defer to H2) and the HTTP server transport (S2 — local-only stdio is the 80% use case for v1). Decision for Kuba.

---

## 6. Open Questions for Further Design

Sketch deliberately leaves these unresolved — flag for the real design phase.

1. **OAuth 2.1 timeline.** Strict spec-compliance for Streamable HTTP requires it; bearer is widely tolerated in 2026 but not future-proof. When does pragmatic become unacceptable?

2. **Tool-level vs server-level capability prompts.** Coarse (one capability per MCP server) is the easy path; fine (per-tool prompts) is the secure path. What's the user threshold where fine becomes mandatory?

3. **MCP server-installed-as-plugin vs separate concept.** Does an MCP server show up in the same "Plugins" tab in Settings, or a separate "MCP Servers" tab? Argues for unified UX (it *is* a plugin); argues against in that MCP servers have different lifecycle expectations (user usually trusts a smaller set).

4. **Versioning the MCP spec dependency.** MCP rev'd ~3 times in 2025. What's our update cadence — chase the spec as it changes, or pin to a stable revision and lag?

5. **`ai_callable` vs `mcp_safe` — same field or two?** The AI agent and an external MCP client are both "AI invokers" at the surface layer. Does the distinction matter, or is it one boolean (`callable_by_ai: true`)? My guess: same field. Confirm before locking schema.

6. **Tool schema transformation rules.** MCP `inputSchema` is full JSON Schema; RIM's `CommandArgumentDto` is a typed enum (`String | Integer | UserId | ...`). What happens to `oneOf`, `anyOf`, nested objects, refs? Likely we restrict to a JSON Schema subset on import and reject (or partially register) tools with unsupported schemas.

7. **Subprocess lifetime / pooling.** One subprocess per MCP server, kept warm? Or spawn-per-call? Spec implies long-lived. Performance and resource footprint at scale (10+ MCP servers installed) is unclear.

8. **MCP Apps UI lifecycle.** When does the iframe close? On next user message? On tool result expiry? On explicit dismiss? Spec is loose here.

---

## 7. Risks

### 7.1 Spec churn (HIGH)

MCP shipped late 2024, rev'd 2025-03, 2025-06, 2025-11; MCP Apps shipped 2025-11 (preview) and 2026-01 (official). **The spec is moving fast.** If we lock to a 2026-04 revision and the next rev breaks compat, we re-do work.

Mitigation: ship the *minimum* MCP surface that a real MCP server needs, leave room in our DTOs for spec-driven additions, watch the changelog. Don't get clever with extensions.

### 7.2 Security surface (HIGH — Black Widow review)

Three distinct attack surfaces:
- **Inbound (host):** malicious or compromised MCP servers running with user privileges.
- **Outbound (server):** untrusted external clients invoking RIM commands.
- **MCP Apps iframes:** untrusted UI rendering inside RIM (mitigated by sandbox, but the postMessage channel is a new RPC surface).

**Black Widow review must cover:**
- Plugin trust statement UX (G-MCP-1, §2.5).
- Subprocess privilege model (G-MCP-2, §2.5).
- HTTP transport without OAuth — risk acceptance (§3.2b).
- `mcp_safe` allowlist policy and per-command audit (§3.3).
- iframe sandbox attribute review (already strict, but MCP Apps spec adds a new RPC surface).

### 7.3 Performance — subprocess fan-out (MEDIUM)

10 MCP servers × keep-alive subprocesses = 10 long-lived processes per RIM session. Memory and FD costs scale linearly. Cold-start (spawn-per-call) is too slow for interactive use.

Mitigation: idle timeout per subprocess (kill after N minutes of no calls, respawn on next call). Cap concurrent live subprocesses (e.g., 20). LRU eviction.

### 7.4 Ecosystem fragmentation (LOW–MEDIUM)

Multiple AI workspaces are racing to define "AI plugin" standards. MCP appears to have won, but if a competing standard (LangChain, OpenAI's function-calling toolkits) gains traction, we may be implementing a niche spec.

Mitigation: low. Bet on MCP. If we're wrong, the host crate is small enough to swap.

### 7.5 Strategic risk: late entrant penalty (MEDIUM)

If we wait, every MCP-capable workspace becomes a competitor. If we ship, we're a peer to Cursor/Claude Desktop/etc. — they're not consciously building "comms" features, so we own the comms slot in their tool ecosystem.

Window is ~6–12 months. After that, competing products will have shipped MCP integrations and the differentiation evaporates.

---

## 8. Recommendation for Kuba

**Build the cheapest viable v1 (H1 + S1 only):** RIM as MCP host (stdio plugins) plus RIM as MCP server (stdio bridge binary). ~4–5 weeks single-dev calendar.

**Sequencing:**
1. Ship CLI v1 first (Task #12). Establishes API token + bearer auth.
2. Ship AI Agent reframe (Task #9). Establishes `ai_callable`/`mcp_safe` field and unified registry.
3. *Then* ship MCP H1 + S1. Rides on (1) and (2).

**Defer:** MCP Apps UI (H2, +M effort), Streamable HTTP server (S2, +M), OAuth (S3, L–XL), sandbox hardening (H3, L). Most are non-blocking polish.

**Don't ship if:** Kuba decides RIM's competitive position is better as a *closed* programmable workspace (own commands only, no external tool ecosystem). That's a coherent strategy too — it just leaves the platform play on the table. My read of the strategic recon (Task #4) says no, but the call is Kuba's.

— Falcon

---

## Sources

- [MCP Apps blog — January 2026 official release](https://blog.modelcontextprotocol.io/posts/2026-01-26-mcp-apps/)
- [MCP Apps overview](https://modelcontextprotocol.io/extensions/apps/overview)
- [MCP Apps GitHub repo](https://github.com/modelcontextprotocol/ext-apps)
- [MCP Transports spec (2025-11-25)](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
- [Stack Overflow — MCP authentication and authorization (Jan 2026)](https://stackoverflow.blog/2026/01/21/is-that-allowed-authentication-and-authorization-in-model-context-protocol/)
- [AWS Builder — MCP transport mechanisms STDIO vs Streamable HTTP](https://builder.aws.com/content/35A0IphCeLvYzly9Sw40G1dVNzc/mcp-transport-mechanisms-stdio-vs-streamable-http)
