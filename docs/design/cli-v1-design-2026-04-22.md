# RIM CLI v1 — Surface Design (Design Sketch)

**Author:** Falcon (Sam Wilson, recon)
**Date:** 2026-04-22
**Audience:** Tony Stark (CTO), Kuba (owner), Doctor Strange (backend lead, future implementor), Black Widow (security review touchpoints flagged)
**Status:** Design sketch — no code. Implementation goes through Strange + Hawkeye review afterward.
**Revised:** 2026-04-22 (post-spawn correction from Tony) — CLI lives in its own repository, NOT as a workspace member of rim-server. See §0.1 below for the architectural implications.

> "I do what he does, just slower." — Sam
> The CLI is RIM's keyboard-first identity made real for terminals. If we can't `curl` our chat, the thesis is incomplete.

---

## 0. Executive Summary

CLI v1 is a thin, scriptable Rust binary (`rim`) shipped from a **separate repository** (`~/RustroverProjects/rim-cli`), consuming RIM's public GraphQL API as any third-party tool would. It reuses `CommandService::execute_command` over the wire (not via crate import) for all action verbs and ships three top-level user-facing commands: `rim send`, `rim list`, `rim run`. Auth is **API-token only for v1** (long-lived, scoped, Settings-issued, server stores hashes — same pattern as session tokens at `rim_auth/src/session_token.rs:6-18`). No interactive cookie login in v1 — that's pure complexity for negative gain when the user is already in a terminal.

Effort: **M (~2 weeks for one Rust dev).** No new domain concepts; the entire surface is a CLI projection of contracts the server already exposes. Two pieces of new server work required: (1) `ApiToken` entity + `auth.create_api_token` mutation, (2) `Authorization: Bearer` header acceptance in `rim_server/src/graphql_support.rs`. CLI itself is mostly clap + reqwest + serde plumbing in a new standalone repository with independent CI and `cargo install rim-cli` distribution.

## 0.1 Architectural Anchor: Separate Repository

**This design assumes the CLI lives in a new top-level repository `~/RustroverProjects/rim-cli`, not as a workspace member of rim-server.** All architectural choices below flow from that constraint.

**Why this is the right call (and not just "Tony said so"):**

1. **The CLI cannot import internal crates.** No `rim_services`, no `rim_core`, no `rim_infra_db`. Whatever the CLI needs MUST be in the public GraphQL schema. If something is conveniently accessible via in-process call but missing from the schema, that's a *backend gap to file*, not a CLI shortcut. **The CLI keeps the API honest.**
2. **`cargo install rim-cli` is a real story.** Standalone crate, publishable to crates.io independently of server release cadence. Users install the CLI without cloning the entire backend repo or running Docker.
3. **Independent versioning & release.** CLI 1.0 stays compatible with rim-server 1.0–1.x as long as the schema doesn't break — same way `gh` stays compatible with `github.com`. Schema break = CLI major bump.
4. **Sharper auth model.** A long-lived API token is now not just a "convenience" — it's the *only* way. The CLI can't sniff a session cookie out of the desktop app's process or Stronghold (different process, different repo, different binary). Server **must** ship a Settings UI for token issuance. Black Widow's territory.
5. **Live GraphQL contract validator.** CLI's CI fetches `/api/sdl` from a pinned rim-server version and regenerates types — exact same pattern as `rim-ui`'s codegen (per `rim-ui/CLAUDE.md:74-117`). When backend resolvers change, CLI build catches it.

**What stays the same as the original architectural intent:** still a thin transport client consuming GraphQL. Still uses `executeCommand` mutation as the universal action path. Still M effort. The repo split actually makes the design *cleaner*, not larger.

Three sharp edges for Black Widow's review (flagged inline below): API tokens are bearer credentials with no CSRF protection (G-AUTH-1), token storage on disk is OS-keychain *or* perm-locked file (G-STORE-1), dangerous-command confirmation needs an explicit override flag with audit logging (G-CONFIRM-1).

---

## 1. Goals & Non-Goals

### Goals (v1)

- **Scriptable access to the workspace** — usable from shell scripts, cron, Makefiles, CI.
- **GraphQL contract validator** — every CLI release is a smoke test of the public API. If a frontend codegen breaks, the CLI release notices.
- **"You can curl this chat" story** — `rim send #ops "deploy done"` in a release script is the demo.
- **Headless-friendly** — no terminal interactivity required for the happy path; reads input from flags, args, stdin.
- **Cross-platform** — Linux + macOS first-class. Windows targeted but not blocking v1 (see §10).

### Non-Goals (v1)

- **TUI mode** — that's v2 (`ratatui`). CLI v1 stays one-shot, exit-after-action. (Per ROADMAP.md:370–376.)
- **Real-time subscription consumption** — `rim watch #alerts` is in the ROADMAP teaser (`ROADMAP.md:364`) but slips to v1.5 after CLI ships and we've validated the auth model under load. Subscriptions need WebSocket auth for bearer tokens, which is its own review.
- **Multi-user / multi-account session juggling on one host** — single active profile per shell, switched via `RIM_PROFILE` env var.
- **Plugin invocation from CLI** — plugins need the desktop runtime; CLI can `run` server-side commands only. (See §3.3 dispatch.)
- **Bot/webhook identity** — out of scope. Use a service-account user with an API token.

---

## 2. Auth Model

### 2.1 The decision: **API tokens only in v1**

Three options were considered. Picking **A** with an explicit "no B" rationale.

| | Option A: Long-lived API token | Option B: Cookie-based interactive login | Option C: Both |
|---|---|---|---|
| UX (scripts) | ★★★ — paste once, use forever | ★ — needs interactive flow once | ★★★ |
| UX (humans) | ★★ — must visit Settings UI | ★★★ — `rim login` is muscle memory | ★★★ |
| Storage complexity | Low (one secret) | Medium (cookie jar + CSRF token) | High |
| CSRF safety | N/A (bearer not subject to CSRF) | Replicates browser CSRF dance (`graphql_support.rs:108-135`) | High surface area |
| Revocation story | Per-token revoke in Settings | Per-session revoke (already exists via `auth.logout`) | Both |
| Code path reuse | Add `Authorization` header path | Reuse existing cookie path | Both |

**Why A and not B for v1:** B forces us to ship a CSRF-aware HTTP client in the CLI just so the CLI can mimic a browser. The whole point of a CLI is *not* being a browser. B also reintroduces the same `rim_csrf` cookie + `X-CSRF-Token` header double-submit that exists today purely to defeat browser cross-origin attacks (`graphql_support.rs:108-135`); a Rust binary doesn't have an origin. A bearer-token path avoids that entire mechanism.

**Why not C:** every "both" CLI I've seen ends up with users picking one and docs maintaining two flows. Lock down to A in v1, revisit in v2 if real users beg for `rim login`.

### 2.2 API token shape (server-side work)

New `ApiToken` entity (separate from `Session`), persisted in PostgreSQL via SeaORM:

```
api_tokens
  id              ApiTokenId (model_id! macro)
  user_id         UserId
  name            String          -- human label, "laptop-cli", "ci-deploy"
  token_hash      String          -- SHA-256 hex, same hash_token() at rim_auth/src/session_token.rs:13-18
  prefix          String          -- "rim_pat_" + first 6 chars of raw token, for display only
  created_at      DateTime<Utc>
  last_used_at    Option<DateTime<Utc>>
  expires_at      Option<DateTime<Utc>>  -- null = never
  revoked_at      Option<DateTime<Utc>>
  scope           String          -- v1: only "full_user" (placeholder for future scoping)
```

Token format on the wire: `rim_pat_<43-char-base64url>`. Identical generation routine as `generate_session_token()` at `rim_auth/src/session_token.rs:6-11`, just different prefix. **This is a deliberate echo** — same crypto, same length, same entropy budget (256 bits), so the security review converges fast.

**New service surface in `rim_services::auth::service::AuthService`:**
- `create_api_token(actor, name, expires_at: Option) -> ApiTokenCreateResultDto { token_dto, raw_token }`
- `list_api_tokens(actor) -> Vec<ApiTokenDto>` (no raw token, ever — only prefix + metadata)
- `revoke_api_token(actor, token_id)`

**New `RequestAuthenticator` path:** `rim_server/src/graphql_support.rs:149-164` already extracts auth from a session cookie. Add a parallel branch: if `Authorization: Bearer rim_pat_…` header is present, hash and look up in `api_token_repo` instead. Returns the same `RequestAuthResultDto`. Same `Actor` injected into GraphQL context downstream — *the entire schema is bearer-aware for free.*

### 2.3 CSRF: **bearer tokens are exempt** [G-AUTH-1 — Black Widow review]

Today's CSRF check (`rim_server/src/graphql_support.rs:108-135`) protects cookie-based sessions from cross-origin form posts. Bearer tokens in `Authorization` headers are immune to that attack class — a malicious site can't make a browser send a custom `Authorization` header cross-origin (CORS preflight kills it, and there's no ambient credential to steal anyway).

**Implementation rule:** in `enforce_csrf_for_batch`, skip CSRF validation when the request authenticated via bearer token (not cookie). This requires `extract_actor_and_session_id` to also return *which auth method was used* so `enforce_csrf_for_batch` can branch.

**For Black Widow:** confirm that skipping CSRF on bearer-authenticated requests is correct. My read: yes, this is industry-standard (GitHub's REST API, Stripe's API, every OAuth-bearer API in existence). Surface to confirm: `rim_server/src/graphql_support.rs:57-69` (`enforce_csrf_for_batch`).

### 2.4 Token issuance flow

1. User opens RIM desktop → Settings → "API Tokens" tab (new).
2. Clicks "Generate new token", names it ("laptop-cli"), optionally sets expiry.
3. Confirmation modal (this is a `dangerous` action — token grants full account access).
4. Server runs `auth.create_api_token` mutation → returns raw token *exactly once*.
5. UI displays raw token with "Copy to clipboard" + warning "this is the only time you'll see this token."
6. User pastes into `rim auth set-token` (CLI side).

No CLI `rim login` interactive flow for v1. Settings UI is the issuance point.

---

## 3. Command Surface (v1)

### 3.1 The three verbs

```
rim send <CHANNEL> [MESSAGE...]   # send a message
rim list <RESOURCE>                # list channels | members | incidents | macros | tokens
rim run <COMMAND-ID> [ARGS...]    # invoke any registry command (built-in, user, macro)
```

Plus three thin auth/config helpers:
```
rim auth set-token                 # paste token from Settings UI
rim auth whoami                    # print current user + active profile
rim auth logout                    # delete local token (does NOT revoke server-side)
```

That's it. Six top-level subcommands. The product is in `rim run` — that one verb dispatches to all 37+ built-in commands plus every user macro the workspace has, because **the CLI doesn't redefine commands; it speaks the registry.** This is the same insight from the AI Agent audit (Task #6): one source of truth.

### 3.2 `rim send` — the killer demo

```
rim send <CHANNEL> [MESSAGE...]

ARGS:
  <CHANNEL>   #channel-name | @username | conv-id (UUID)
  [MESSAGE]   Message text. If absent, read from stdin.

FLAGS:
  -t, --thread <MESSAGE_ID>     Reply in thread
  -f, --file <PATH>             Attach file (repeatable)
  --as <PROFILE>                Override active profile
  -o, --output <FORMAT>         json | yaml | plain (default: plain — prints message ID)
  -y, --confirm                 Skip dangerous confirmation
  -q, --quiet                   Suppress non-essential output

EXAMPLES:
  rim send '#ops' 'Deploy complete'
  echo "build failed" | rim send '#alerts'
  rim send '@alice' -f ./report.pdf 'Here is the report'
  ls -la | rim send '#dev' --thread 0192abcd-…
  rim send '#release' "$(git log -1 --pretty=format:'Released %s')"
```

**Under the hood:** resolves `<CHANNEL>` to a `ConversationId` via existing GraphQL queries (`channels` for `#name`, DM lookup for `@user`, direct UUID parse otherwise), then invokes `chat.post_message` via `executeCommand` mutation (`rim_api/src/mutation.rs:752-773`) — *not* a separate `sendMessage` mutation. Same path the agent will use post-reframe. **Audit trail unified.**

### 3.3 `rim list` — readonly resource enumeration

```
rim list channels              # all channels in active workspace
rim list members [-c CHANNEL]  # all workspace members, or channel-scoped
rim list incidents [--status open|closed] [--severity SEV_1..SEV_4]
rim list macros                # user macros + built-in commands
rim list commands [-q SEARCH]  # full command registry (incl. plugin commands)
rim list tokens                # API tokens for current user
rim list workspaces            # workspaces this user belongs to
```

Output defaults to a compact table for terminals (detected via `isatty`), JSON when piped. `--output json` always forces JSON. JSON is canonical: `{ items: [...], page_info: { has_next, end_cursor } }` mirroring GraphQL connection shape.

**Pagination:** `--limit N` (default 50, max 500) and `--cursor C`. Cursor format is opaque to the CLI — server-defined per `docs/pagination.md`.

### 3.4 `rim run` — the registry as CLI

```
rim run <COMMAND-ID> [--ARG=VALUE ...] [-y] [--pipe]

EXAMPLES:
  rim run chat.post_message --conversation-id=… --body='hi'
  rim run state.set --key=deploy.last --value=v1.2.3
  rim run user.my-deploy-macro --env=staging
  rim run state.incr --key=incidents.count
  rim run plugin.youtrack.create_ticket --title='bug' --description='...'
```

**Args mapping:** Each `CommandArgumentDto` (typed: `String | Integer | UserId | ConversationId | MessageId | …` per `rim_services/src/command/dto.rs:43-53`) maps to a `--kebab-case-name=value` flag. Type validation happens server-side (we don't reimplement it) — CLI is a dumb projection.

**Required-context auto-injection:** the CLI can't supply `selected_message` or other UI-side context, so commands with `required_context` containing `selected_message` will return a `ServiceError::Validation("missing context: selected_message")` and the CLI surfaces that as exit code 65 with a hint: *"this command requires UI context — run from desktop."* Per audit Part B.4, `required_context` is a known field (`CommandDefinitionDto.required_context`).

**Macros:** named macros (`MacroService::get_macro_by_name`, `rim_services/src/macro_def/request.rs:48-54`) resolved by leading `user.` prefix or via `--macro-name=` flag. Already exists server-side.

**Pipes (deferred from v1):** `rim run a | rim run b` works via shell, but in-process command piping (`is_pipe: true`, `rim_services/src/command/request.rs:19-28`) is a v1.5 feature. v1 just dispatches single commands.

### 3.5 Exit codes (UNIX convention)

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Generic error |
| `2` | Misuse (bad CLI args) |
| `3` | Auth missing/invalid |
| `4` | Permission denied (server `ServiceError::PermissionDenied`) |
| `5` | Not found (server `ServiceError::NotFound`) |
| `6` | Conflict (server `ServiceError::Conflict`) |
| `64` | Validation (server `ServiceError::Validation`) |
| `65` | Missing required context (CLI-level) |
| `69` | Server unavailable (network / 5xx) |
| `77` | Dangerous action without `-y` |
| `78` | Configuration error (no profile, malformed config) |

These align loosely with `sysexits.h`. The point is: scripts can branch on them.

### 3.6 Output format contract

Every command supports `-o/--output {plain|json|yaml}`. **JSON is the stable contract; plain is the human-friendly default for TTYs.** Schema for JSON output is documented per command in `--help` and is a versioned contract: breaking changes bump the CLI's major version.

For `rim run`, JSON output is always the raw `CommandResult.output_json` from the server — we don't re-shape it. Same payload an agent would consume. **Same audit trail.**

---

## 4. Architecture

### 4.1 Repository layout — `rim-cli` is its own repo

**Location:** `~/RustroverProjects/rim-cli` (new repo, not yet created).

The CLI does **not** sit in the rim-server workspace. It's an independent Rust project that consumes RIM's public GraphQL surface — same way `gh` consumes GitHub's API, same way a third-party rim plugin would. **No internal crate imports** (`rim_services`, `rim_core`, `rim_infra_db` are all unreachable by design).

**Day-1 repository skeleton:**

```
rim-cli/
  Cargo.toml                 # standalone, [package] name = "rim-cli", bin name = "rim"
  Cargo.lock
  rust-toolchain.toml        # pinned to match rim-server (edition 2024)
  README.md                  # install instructions, quickstart, link to docs
  LICENSE
  schema.graphql             # SDL pinned to a specific rim-server version, regenerated via CI
  build.rs                   # OPTIONAL — regenerates types from schema.graphql at build time
  src/
    main.rs                  # entry, clap dispatch, subcommand routing
    lib.rs                   # exposes internals for integration testing only
    config.rs                # ~/.config/rim/config.toml load/save
    auth/
      mod.rs                 # token storage trait
      keyring.rs             # OS keychain adapter (macOS / Linux / Windows)
      file_fallback.rs       # plaintext-mode-0600 fallback
    transport/
      mod.rs                 # GraphQL client trait
      reqwest_client.rs      # reqwest implementation (Authorization: Bearer)
      types.rs               # generated types (if codegen) or hand-written request structs
    output/
      mod.rs                 # OutputFormatter trait
      plain.rs               # tabwriter-based human format
      json.rs                # serde_json passthrough
      yaml.rs                # serde_yaml passthrough
    error.rs                 # CliError, exit code mapping per §3.5
    cmd/
      send.rs
      list.rs
      run.rs
      auth.rs                # set-token / whoami / logout
      completion.rs          # clap_complete subcommand
    resolve.rs               # #channel → ConversationId, @user → UserId
  tests/
    e2e.rs                   # feature-gated, spins up local rim-server (see §7.2)
    snapshot/                # insta snapshots of --help output
  .github/
    workflows/
      ci.yml                 # lint + unit tests on every PR
      schema-sync.yml        # weekly: re-fetch schema.graphql from pinned server commit, open PR
      release.yml            # cargo publish + cross-platform binary release on tag
  docs/
    install.md
    auth.md
    cookbook.md              # 10 useful one-liners
```

**Why this layout (and not deeper nesting):** the CLI is genuinely small. ~20 source files, ~3-4k LOC at v1. Over-modularization at v1 makes future v2 (TUI extraction) harder, not easier — the v2 split (per §8) wants to extract whole files, not unscramble nested modules.

### 4.2 The "no internal imports" discipline (the load-bearing constraint)

The architectural rule that makes everything else clean:

> **`rim-cli` may NOT depend on `rim_services`, `rim_core`, `rim_infra_*`, `entity`, or any internal rim-server crate. It depends only on public crates (clap, reqwest, etc.) and on the GraphQL schema published by the running server.**

Three reasons:

1. **Embedding the backend in the CLI is silly.** It would force a DB driver, sea-orm, the entire migration crate transitively into `cargo install rim-cli`. Rejected.
2. **Bypassing the auth surface is a security risk.** In-process command invocation skips `RequestAuthenticator`. The CLI must come through the front door.
3. **The CLI keeps the API honest** (the strategic payoff). If a backend feature works only via in-process service trait calls but isn't reachable through GraphQL, the CLI exposes that gap. Filed as a backend issue, not a CLI workaround.

**Concrete consequence: shared types.** The CLI cannot reuse `UserId`, `ConversationId`, etc. from `rim_core::*::model`. It either:

- **(a) re-derives them locally** as transparent `Uuid` wrappers (`pub struct ConversationId(Uuid);` with `serde(transparent)`), or
- **(b) takes them as opaque `String` from GraphQL** and never parses them (just round-trips them as references).

Recommend **(b) for v1.** The CLI doesn't need to validate IDs locally — the server does that. Treating IDs as opaque strings means schema evolution (e.g., switching from UUIDv7 to ULID) doesn't break the CLI. *The CLI doesn't care what's in the string; it only cares what the server thinks of it.*

### 4.3 Schema sync: GraphQL is the contract

**Source of truth:** `rim-server` exports SDL via `GET /api/sdl` (per `rim-server/CLAUDE.md:107-128`).

**Sync strategy** (mirrors `rim-ui`'s codegen pattern at `rim-ui/CLAUDE.md:74-117`):

1. `rim-cli/schema.graphql` is a **pinned snapshot** of the SDL, committed to the repo.
2. CI workflow `schema-sync.yml` runs weekly (or on-demand): `curl https://chat.example.com/api/sdl > schema.graphql`. If the file changed, opens an automated PR titled "chore: sync schema to rim-server commit \<sha\>".
3. PR runs full test suite + e2e; passing → human merges + new CLI version released.
4. Breaking schema changes (removed fields, changed types) → CLI fails to build → the PR explicitly forces a CLI code change before merge.

**Pinning protocol:** `schema.graphql` header comment records the rim-server git SHA it was fetched from. CLI's `--version` output prints both `rim-cli 0.X.Y (schema rim-server@<sha>)`.

**GraphQL client choice — recommendation: `cynic` over `graphql-client` over raw `reqwest`.**

| Option | Pros | Cons | Verdict |
|---|---|---|---|
| **`cynic`** | Type-safe queries written as Rust structs. Errors at compile time on schema mismatch. Active 2026 maintenance. No build-time codegen — uses `cynic-codegen` proc-macro at compile, drives off `schema.graphql`. | Steeper learning curve. Less common than `graphql-client`. | **Recommended.** Type safety from the schema-pinning approach is the whole point. |
| `graphql-client` | Codegen via `query.graphql` files, mature, widely used. | Build-time codegen step; macros around `gql_query!` can be opaque. | Acceptable fallback if `cynic` proves friction. |
| Raw `reqwest` + hand-typed structs | Zero magic, easy to understand, no dep. | Hand-typing every response is tedious AND error-prone — *defeats the "live contract validator" purpose*. | Rejected. Without compile-time schema check, the CLI no longer keeps the API honest. |

**Use `cynic` v3.x (or current stable). Hand-typing was my v1-spawn recommendation; revising — given the no-import constraint, compile-time schema verification matters more than code minimalism.**

### 4.4 Module shape (revised summary)

The §4.1 tree above lays out the module structure. Key public surface:

- **`transport::GraphqlClient` trait** — abstracts over the reqwest client; lets unit tests inject a mock.
- **`auth::TokenStore` trait** — abstracts keychain vs file backends; lets unit tests inject in-memory.
- **`output::Formatter` trait** — Plain/Json/Yaml impls.
- **`cmd::Subcommand` trait** — each subcommand implements `async fn run(&self, ctx: &CliContext) -> Result<ExitCode, CliError>`.

These four traits are the seams §8 wants preserved for v2 TUI extraction.

### 4.5 Dependencies (recommended)

| Crate | Why |
|---|---|
| `clap` (derive) | Standard. Derive style for type-safe arg parsing + auto help. |
| `reqwest` (rustls-tls) | HTTP client. No openssl dep — important for static binary distribution. |
| `cynic` + `cynic-codegen` | Type-safe GraphQL client with compile-time schema verification. **Replaces hand-typed structs (revised from v1-spawn recommendation — see §4.3).** |
| `tokio` (rt-multi-thread) | Runtime. |
| `serde` / `serde_json` / `serde_yaml` | Output formatters and config. |
| `toml` | Config file format. |
| `keyring` | Cross-platform OS keychain (macOS Keychain, Linux Secret Service, Windows Credential Manager). |
| `directories` | XDG-compliant config dir (`~/.config/rim/` on Linux, `~/Library/Application Support/rim/` on macOS, `%APPDATA%\rim\` on Windows). |
| `is-terminal` | TTY detection for table-vs-JSON default output. |
| `tabwriter` | Plain table rendering. Lean and standard. |
| `anyhow` | Error wrapping inside `main`. CLI doesn't need rich error types — it needs good messages. |
| `clap_complete` | Shell completion generation per §6.3. |

**Notable absences:**
- No `tokio-tungstenite`. WebSocket subscriptions are out of scope for v1 (per §1).
- No internal rim-server crates (per §4.2).
- No `Stronghold` crate. CLI uses OS keychain directly via `keyring`. Stronghold is a desktop-app concern — different process, different repo.

### 4.6 Reuse pattern (over the wire)

| CLI subcommand | GraphQL operation | Server-side handler (informational only) |
|---|---|---|
| `rim send` | `mutation { executeCommand(input: { commandId: "chat.post_message", ... }) }` | `CommandService::execute_command` |
| `rim list channels` | `query { channels(...) }` | `ConversationService` |
| `rim list incidents` | `query { listIncidents(...) }` | shipped Task #1 |
| `rim list commands` | `query { commands(surface: PALETTE, ...) }` | `CommandService::list_commands` |
| `rim list macros` | `query { macros }` | `MacroService::list` |
| `rim list tokens` | `query { apiTokens }` | **NEW** — `AuthService::list_api_tokens` |
| `rim run <id>` | `mutation { executeCommand(input: ...) }` | `CommandService::execute_command` |
| `rim auth set-token` | (none — local only) | n/a |
| `rim auth whoami` | `query { me { id, username } }` | `UserService` |

**Single backend execution path = `CommandService::execute_command` over GraphQL.** Aligns with the AI Agent reframe (audit Part E.2). The CLI and the AI Agent are the two non-UI consumers of the unified registry — both validate the same design.

The "server-side handler" column above is for the reader's orientation only — the CLI does not know or care what handler runs. It only knows the GraphQL operation name and shape.

### 4.7 Cross-repo coupling discipline

Question: when rim-server bumps a query, how does CLI v1.0 stay compatible?

**Three-tier compatibility policy** (proposed):

1. **Additive schema changes** (new fields, new query, new optional input field) — CLI ignores them; no compat issue. Update `schema.graphql` snapshot opportunistically.
2. **Backwards-compatible behavior changes** (server adds validation, server returns more in an existing field) — CLI keeps working; integration tests catch behavioral regressions.
3. **Breaking changes** (removed field, renamed type, changed argument type) — CLI build fails the moment `schema.graphql` is updated to the new SDL. *That's the trip-wire.* CLI requires a coordinated PR (server PR + CLI PR + version bump on both).

**Compatibility window:** CLI 1.x compatible with rim-server 1.x as long as no GraphQL breaking change ships. CLI 2.0 = response to a server breaking change.

**Version exposure:** `rim --version` prints `rim-cli 1.2.3 (schema rim-server@abc1234)`. Server's GraphIQL or `/api/sdl` should likewise expose the schema commit hash so users can verify compatibility.

**No backend SDK published.** The CLI is the de-facto reference Rust client. If a third party wants to build their own Rust GraphQL consumer, they pin to the same `schema.graphql` and pick their own client crate. We don't ship a `rim-sdk` for v1 — too early, premature abstraction.

---

## 5. Config

### 5.1 File: `~/.config/rim/config.toml`

XDG-compliant via the `directories` crate. Linux: `~/.config/rim/config.toml`. macOS: `~/Library/Application Support/rim/config.toml`. Windows: `%APPDATA%\rim\config.toml`.

```toml
default_profile = "personal"

[profile.personal]
endpoint = "https://chat.example.com/api"
user_id  = "0192…"           # cached, for fast `rim auth whoami` without a roundtrip
token_keyring_id = "rim:personal"  # opaque OS-keychain handle (see §5.3)

[profile.work]
endpoint = "https://rim.work-corp.internal/api"
user_id  = "0193…"
token_keyring_id = "rim:work"

[profile.dev]
endpoint = "http://localhost:8080/api"
user_id  = "0194…"
token_keyring_id = "rim:dev"
```

**Profiles:** `--profile <NAME>` flag, `RIM_PROFILE` env var, or `default_profile` from config. CLI evaluates in that precedence order (flag > env > default).

### 5.2 Environment variables

| Var | Purpose |
|---|---|
| `RIM_PROFILE` | Override default profile |
| `RIM_TOKEN` | Bypass keychain entirely — useful for CI |
| `RIM_ENDPOINT` | Override profile endpoint |
| `RIM_OUTPUT` | Default output format (`json`, `yaml`, `plain`) |
| `RIM_NO_COLOR` | Disable ANSI color (also respects `NO_COLOR` per https://no-color.org) |
| `NO_COLOR` | Standard convention, also honored |
| `RIM_CONFIG_DIR` | Override config directory (testing, sandboxing) |

**Precedence:** CLI flag > env var > config file > built-in default. Standard layered config pattern.

### 5.3 Token storage [G-STORE-1 — Black Widow review]

**Primary:** OS keychain via the `keyring` crate. Per-profile keyring entry: service `"rim"`, account `<profile-name>`. macOS Keychain / Linux Secret Service (gnome-keyring/kwallet) / Windows Credential Manager.

**Fallback** (when keyring unavailable, e.g., headless Linux server, Docker container): plaintext file at `~/.config/rim/tokens/<profile>.token` with mode `0600`. CLI checks `stat()` on read — refuses to load if mode != `0600` or owner != current user. Same posture as `~/.ssh/id_rsa`.

**For Black Widow:** confirm fallback file mode check is sufficient. Compared options:
- (a) Refuse to start without keyring → blocks CI/Docker entirely. Rejected.
- (b) Encrypt the file with a passphrase from env → adds prompt friction; passphrase ends up in env anyway. Rejected.
- (c) Plain file with strict mode + owner check → matches `gh auth`, `aws cli`, `kubectl`. **Accepted.**

GitHub CLI follows the exact same pattern: keychain primary, plaintext fallback. ([cli.github.com/manual/gh_auth_login](https://cli.github.com/manual/gh_auth_login))

**Tokens are NEVER printed in normal CLI output.** Even `rim auth whoami` only shows `prefix` (first 6 chars of token, displayed as `rim_pat_abc123…`). To re-display a full token, the user must regenerate it in Settings — *the server never returns the raw token after creation*, same posture as session tokens at `rim_auth/src/session_token.rs:6-11`.

### 5.4 CI / automation pattern

```bash
# In CI:
export RIM_TOKEN="$RIM_DEPLOY_TOKEN"     # from secret store
export RIM_ENDPOINT="https://rim.corp/api"
export RIM_OUTPUT=json
rim send '#deploys' "Build $CI_BUILD_NUMBER complete"
```

No config file required. No keychain required. Pure env-driven — works in any container or CI runner.

---

## 6. UX

### 6.1 Help text

**Use `clap` derive.** Builder is more flexible but derive gives you 90% with strict types and forces consistent doc-comments → `--help` text. Builder is overkill for ~6 subcommands.

`rim --help` flow: top-level lists the 6 verbs with one-line descriptions + a "GETTING STARTED" footer pointing at `rim auth set-token`. `rim <verb> --help` shows full per-verb help with examples in a `EXAMPLES:` block (clap supports this via `after_help`).

### 6.2 Error UX

- **All errors → stderr.** stdout is reserved for command output. This is sacred for piping.
- **Exit codes per §3.5.**
- **Color:** Errors red, warnings yellow, success green. Auto-disabled when stdout is not a TTY or `NO_COLOR` is set.
- **Error message shape:** one-line summary + optional indented "hint" line.
  ```
  error: permission denied
    hint: your token may have expired — run `rim auth set-token` to update
  ```
- **Server errors:** GraphQL `ServerError.code` maps to exit code (per §3.5). Message comes from server `ServerError.message`. Server-side `request_id` (set in `rim_server/src/server.rs:309-335`) printed at end of error if `--verbose`, so users can include it in bug reports.

### 6.3 Shell completions

`clap_complete` generates Bash / Zsh / Fish / PowerShell completions:

```bash
rim completion bash > /etc/bash_completion.d/rim
rim completion zsh  > ~/.zsh/completions/_rim
rim completion fish > ~/.config/fish/completions/rim.fish
```

Per release, these are also packaged in `dist/completions/` so distro maintainers can install them automatically.

### 6.4 Confirmation flow [G-CONFIRM-1 — Black Widow review]

Per audit Part B.4, the backend honors `confirmed_dangerous: true` in `ExecuteCommandCmd`. CLI maps this to `-y/--confirm`:

- **TTY + dangerous command + no `-y`:** prompt `Run dangerous command 'X'? [y/N]:`. Reads stdin. Default is `N`. Non-`y` answer → exit 77.
- **No TTY + dangerous command + no `-y`:** **refuse outright**, exit 77. Don't auto-confirm in scripts. *This is the load-bearing security rule for CI use.*
- **`-y` flag:** sets `confirmed_dangerous: true` in the mutation. Bypasses CLI prompt. Server still applies its dangerous-action policy (audit trail entry).
- **`--dry-run` (future):** returns the resolved command + args + context that *would* be executed, without invoking it. v1.5 nice-to-have.

**For Black Widow:** confirm that "no TTY + no `-y` → refuse" is correct policy. Alternative is "no TTY → assume `-y`" which I think is dangerous for the `cron` use case where someone forgets `-y` and unintentionally yolo-bypasses.

### 6.5 Output examples

`rim list channels` (TTY default — table):
```
  CHANNEL       TYPE     MEMBERS  UNREAD  ID
  #general      channel       12       0  0192abcd-...
  #ops          channel        4       3  0192bcde-...
  #incidents    channel        7       0  0192cdef-...
  alice         dm             1       1  0192def0-...
```

`rim list channels -o json | jq` (machine-readable):
```json
{
  "items": [
    { "id": "0192abcd-...", "name": "general", "type": "channel", "member_count": 12, "unread_count": 0 },
    ...
  ],
  "page_info": { "has_next": false, "end_cursor": null }
}
```

---

## 7. Test Strategy

### 7.1 Unit tests (in `rim_cli` crate)

- **clap arg parsing per subcommand** — assert that `rim send '#ops' 'hi' --thread X` produces the expected internal `SendCommand` struct.
- **Channel resolver** — `#name` / `@user` / UUID branches. Mock the GraphQL transport.
- **Output formatters** — round-trip a fixture struct through Plain/Json/Yaml; assert JSON is canonical.
- **Config load** — happy path + corrupt TOML + missing file + missing profile.
- **Token storage adapter** — keychain success path + file fallback + mode-check rejection.

### 7.2 Integration tests (feature-gated)

Mirror the `cargo test --features db-tests` pattern at `rim-server/CLAUDE.md:43-46`:

```toml
[features]
e2e-tests = []  # requires running rim-server + valid token in env

[[test]]
name = "e2e"
required-features = ["e2e-tests"]
```

E2E suite runs:
1. Spin up rim-server in CI (already containerized via `Dockerfile`).
2. Bootstrap a test workspace (call existing `BootstrapMutation`).
3. Provision a test API token via the new `auth.create_api_token` mutation.
4. Set `RIM_TOKEN` + `RIM_ENDPOINT` env vars.
5. Run CLI binary as a subprocess: `Command::new("rim").args(["send", "#general", "hi"])`.
6. Assert exit code + stdout + stderr.

Run single-threaded (`--test-threads=1`) like the rest of CI per `CLAUDE.md:38`. **The CLI's e2e test is, in effect, a contract test for the public GraphQL API.** Breaking the schema breaks the CLI build. This is the "live validator" Tony called out in the strategic recon (ROADMAP.md:457-460).

### 7.3 Snapshot tests for help output

`insta` crate. One snapshot per `--help` (top-level + each subcommand). Catches accidental help-text drift across releases. Help text *is* the public contract.

### 7.4 Cross-platform smoke

GitHub Actions matrix: Ubuntu, macOS, Windows. Just builds + runs unit tests; e2e is Linux-only in v1 (Postgres dev container).

---

## 8. v1 → v2 Handoff (Seams, Not Designs)

The v2 TUI (`ratatui`) shouldn't share a binary with the CLI; they share a *core library*. Both still live in the `rim-cli` repo (it converts from a single-binary crate to a workspace). Suggested split for when v2 starts:

```
rim-cli/                       # repo (was: single binary; becomes: workspace)
  Cargo.toml                   # [workspace] members
  rim_cli_core/                # library — extracted: GraphQL client, auth, resolve, errors, types
  rim_cli_bin/                 # binary `rim` — clap dispatch + formatters (today's v1)
  rim_tui_bin/                 # binary `rim tui` — ratatui frontend, depends on rim_cli_core
  schema.graphql               # shared, still pinned
```

This is a *within-repo* refactor; the rim-cli repo stays one entity. The decision to keep CLI separate from rim-server (§4.1) is unchanged.

**Seams to preserve in v1 so the v2 split is mechanical:**

1. **`transport::GraphqlClient` trait is pure** — given `(endpoint, token, op, vars)` returns `Result<T, CliError>`. No CLI-specific concerns. Extracts to `rim_cli_core::transport`.
2. **`resolve.rs` is pure** — `#name → ConversationId` etc., consumes a `GraphqlClient`. Extracts.
3. **`auth::TokenStore` trait is pure** — TUI will need it too. Extracts.
4. **No clap types leak into `cmd/*.rs` action types beyond the derive layer** — internal action structs are framework-agnostic, easy to drive from a TUI command palette later.
5. **`output::Formatter` trait stays in the bin crate** — TUI doesn't need plain/json/yaml formatters; it has its own ratatui rendering. This is the only piece that v2 actively does NOT inherit.

**Subscription readiness for v2:** when WebSocket subs ship, the TUI is the consumer. v1 doesn't need them, but `transport` module should expose its `Authorization`/`Bearer` header builder as a public function so the future `subscription` client can reuse it. One-liner discipline today saves a refactor later.

---

## 9. Comparison

### 9.1 Slack CLI ([slackapi/slack-cli](https://github.com/slackapi/slack-cli))

Slack's CLI exists but is **not** what RIM CLI is. Slack CLI is purely an **app development tool** — `slack create`, `slack run`, `slack deploy`, `slack manifest`. It scaffolds Bolt projects and deploys them; it does not let you send a message or list channels from a script. ([docs.slack.dev/tools/slack-cli](https://docs.slack.dev/tools/slack-cli/))

What Slack *does* have for end-user CLI scripting: nothing official. Third-party tools like [rockymadden/slack-cli](https://github.com/rockymadden/slack-cli) (a bash script that wraps the Web API) fill the gap, badly — no first-class auth, no completions, manual webhook URLs.

**Implication for RIM:** there is genuinely no first-party comms-CLI in the market. Slack noticed (their CLI is now adding AI-agent scaffolding per [recent v4.0 release](https://docs.slack.dev/changelog/2026/04/10/slack-cli/)) but they're still not addressing the "I want to script chat from CI" use case. RIM has clean whitespace.

### 9.2 GitHub CLI (`gh`)

Gold standard. Patterns we copy:
- **Keychain-first token storage with plaintext fallback** ([cli.github.com/manual/gh_auth_login](https://cli.github.com/manual/gh_auth_login)).
- **`GH_TOKEN` env var** for CI — we mirror as `RIM_TOKEN`.
- **Subcommand structure** — verb-first (`gh repo`, `gh issue`), one-thing-per-command. We follow.
- **JSON-or-table output toggle via `--json` / TTY detection**. We follow.
- **`gh auth status` shows current account.** Our `rim auth whoami`.

What gh does that we *don't* in v1:
- Browser-based OAuth flow (`gh auth login --web`) — RIM has no OAuth provider yet. Settings UI issues tokens directly. v2 might add device-code flow.
- Repo discovery from cwd (`gh issue list` works because gh sniffs `git remote`). RIM has no equivalent — workspace is set per profile.

### 9.3 Aider, Claude Code, Gemini CLI

Different shape — these are AI agent CLIs, not transactional API CLIs. But two patterns worth borrowing:

- **`--yes` / `--no-confirm` flags for destructive ops** — universal pattern. We adopt as `-y/--confirm`.
- **Streaming output to terminal with TTY detection** — when output is a TTY, render progressively; when piped, buffer and emit final result. v1 doesn't need streaming (no long-running CLI commands), but v2 TUI mode will.

### 9.4 Discord CLI

Doesn't exist as a first-party tool. Third-party `discord-cli` projects are toy-grade. Same gap as Slack.

### 9.5 Mattermost CLI (`mmctl`)

Closest comparable — Mattermost ships [`mmctl`](https://docs.mattermost.com/manage/mmctl-command-line-tool.html) for *server admin*, not for end users sending messages. Useful precedent: **server admin and user CLI should be separate binaries** (or at least separate top-level subcommands). RIM v1 is end-user only; admin tooling can come later as `rim admin <…>`.

---

## 10. Open Questions / Defer Until Implementation

1. **Windows-native posture.** `keyring` crate supports Windows Credential Manager. ANSI color via `colored` works on Windows 10+ terminals. But: file-mode check (§5.3 fallback) is meaningless on Windows ACLs. Decision: v1 ships Windows binary but logs a warning if falling back to file storage on Windows. v1.5 adds DPAPI-encrypted file storage for the Windows fallback.

2. **Token rotation UX.** v1 has manual rotation only (revoke old in Settings, generate new, paste into CLI). Auto-rotation needs a refresh token, which adds the OAuth machinery we're avoiding. Defer.

3. **`rim run` arg-from-stdin.** Should `--body=-` mean "read body from stdin"? Useful for `cat msg.txt | rim run chat.post_message --conv=… --body=-`. **Yes**, propose adopting from `gh` / `kubectl`. Document: any string arg accepts `-` to mean stdin, only one arg per invocation.

4. **Telemetry / version-check ping.** None in v1. The CLI doesn't phone home. If we want it later, opt-in with explicit consent.

5. **`rim watch` (subscriptions).** Not v1. WebSocket bearer auth is a separate review. Sized roughly as +1 week post-v1.

6. **Plugin command access from CLI.** Built-in + user macros work in v1 because they execute server-side. Plugin commands are sandboxed-iframe + JSON-RPC over stdin/stdout (audit Part B.4) — those need the desktop runtime. CLI surfaces them in `rim list commands` but exits 65 with hint *"plugin commands require desktop runtime"* if you try to run one. Same `required_context` mechanism, new context name like `desktop_runtime`.

---

## 11. Sized Effort

**Backend deltas (~3 days for Strange):**
- `ApiToken` entity + migration (Vision sign-off)
- `AuthService::create_api_token` / `list_api_tokens` / `revoke_api_token`
- `RequestAuthenticator` bearer-token branch in `graphql_support.rs`
- CSRF skip rule for bearer-authenticated requests
- GraphQL surface for token CRUD

**Frontend deltas (~2 days for Spider-Man):**
- Settings → API Tokens tab: list, create, revoke
- Confirmation modal for token creation (one-time-display warning)

**CLI implementation (~1.5 weeks for one Rust dev), in new `rim-cli` repo:**
- Repo bootstrap: `cargo new --bin`, `Cargo.toml`, `rust-toolchain.toml`, README, LICENSE, .gitignore
- `schema.graphql` snapshot from a pinned rim-server commit
- `cynic` codegen wired in (build script or proc-macro)
- transport + auth (keychain + file fallback) + config (TOML) + 6 subcommands
- unit tests + e2e CI feature (Postgres + rim-server in CI container)
- snapshot tests for help text via `insta`
- cross-platform CI matrix (Ubuntu / macOS / Windows; e2e on Linux only)
- shell-completion generation via `clap_complete`
- GitHub Actions workflows: `ci.yml`, `schema-sync.yml` (weekly schema PR), `release.yml` (cross-platform binary + crates.io publish on tag)
- README with `cargo install rim-cli` quickstart, auth setup, cookbook

**Total:** ~2 weeks calendar with parallel work. Fits the **M-effort** estimate from ROADMAP.md:457-460.

---

## 12. The Happy Path — `rim send '#ops' 'deploy done'`

End-to-end of the killer demo:

```
1. SHELL
   $ rim send '#ops' 'deploy done'

2. CLI ENTRY (rim_cli/src/main.rs)
   clap parses → SendCommand { channel: "#ops", message: "deploy done", thread: None, ... }

3. CONFIG LOAD (rim_cli/src/config.rs)
   - Reads ~/.config/rim/config.toml → resolves active profile = "personal"
   - Reads endpoint from profile = "https://chat.example.com/api"
   - Loads token from OS keychain ("rim:personal") → "rim_pat_xxxxx..."

4. CHANNEL RESOLVE (rim_cli/src/resolve.rs)
   GraphQL query:
     query { channels(filter: { name: "ops" }) { items { id } } }
   POST https://chat.example.com/api
   Headers: { Authorization: "Bearer rim_pat_xxxxx..." }
   ← { data: { channels: { items: [{ id: "0192abcd-..." }] } } }

5. EXECUTE COMMAND (rim_cli/src/cmd/send.rs)
   GraphQL mutation:
     mutation {
       executeCommand(input: {
         commandId: "chat.post_message",
         args: { conversationId: "0192abcd-...", body: "deploy done" },
         confirmedDangerous: false,
         isPipe: false
       }) {
         ok
         outputJson
         error
       }
     }
   POST https://chat.example.com/api
   Headers: { Authorization: "Bearer rim_pat_xxxxx..." }
   (No CSRF header — bearer-authenticated, exempt per §2.3)

6. SERVER (rim_server/src/server.rs:123-185 graphql_handler)
   - extract_actor_and_session_id sees Bearer header (NEW BRANCH per §2.2)
   - Looks up api_token by hash → user_id
   - Builds Actor, injects into GraphQL context
   - Skips CSRF check (NEW per §2.3 — bearer-authenticated)

7. RESOLVER (rim_api/src/mutation.rs:752-773)
   require_actor → Ok(user_id)
   Calls CommandService::execute_command(ExecuteCommandCmd { command_id: "chat.post_message", ... })

8. COMMAND DISPATCH (rim_infra_services/src/command/builtin_executor.rs:67-108)
   match "chat.post_message" → MessageService::send_message(SendMessageCmd { ... })
   Returns CommandResult { ok: true, output_json: { message_id: "0192def0-..." } }

9. RESPONSE
   ← { data: { executeCommand: { ok: true, outputJson: { message_id: "0192def0-..." }, error: null } } }

10. CLI RENDER (rim_cli/src/output.rs)
    TTY detection: stdout is a terminal → "plain" formatter
    Plain formatter for executeCommand: prints `0192def0-...` (the message ID) + newline
    Exit 0.

   $ echo $?
   0
```

**Same backend path the AI Agent will use post-reframe.** Same `executeCommand` mutation. Same `CommandService::execute_command`. Same audit trail. **The CLI is the agent's twin without the LLM.**

That's the design. Strange picks it up after Hawkeye review.

— Falcon

---

## Sources

- [Slack CLI v4.0 release notes (April 2026)](https://docs.slack.dev/changelog/2026/04/10/slack-cli/)
- [Slack CLI overview](https://docs.slack.dev/tools/slack-cli/)
- [slackapi/slack-cli on GitHub](https://github.com/slackapi/slack-cli)
- [rockymadden/slack-cli (third-party bash wrapper)](https://github.com/rockymadden/slack-cli)
- [GitHub CLI auth login docs](https://cli.github.com/manual/gh_auth_login)
- [GitHub CLI auth token docs](https://cli.github.com/manual/gh_auth_token)
- [NO_COLOR convention](https://no-color.org)
- [Mattermost mmctl](https://docs.mattermost.com/manage/mmctl-command-line-tool.html)
