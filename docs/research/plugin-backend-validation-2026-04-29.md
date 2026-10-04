# Plugin-Backend Validation + MCP Verdict + Marketplace Discovery

- **Author:** iron-man (Tony Stark, backend architect)
- **Date:** 2026-04-29
- **For:** nick-fury
- **Builds on:**
  - vision's plan at `/home/kuba/RustroverProjects/rim-ui/docs/plans/plugin-system-phase-plan-2026-04-29.md`
  - vision's discovery at `/home/kuba/RustroverProjects/rim-ui/docs/research/plugin-system-status-2026-04-29.md`
  - doctor-strange's competitive memo at `/home/kuba/RustroverProjects/rim-server/docs/research/competitive-landscape-2026-04-29.md`
  - falcon's MCP sketch at `/home/kuba/RustroverProjects/rim-server/docs/design/mcp-bridge-sketch-2026-04-22.md`
  - jakub's Q1 / Q3 / Q4 lock-ins (centralized marketplace as separate service; desktop-only v1; both signed-from-registry AND TOFU sideload)

---

## TL;DR

**Vision's claims, validated:**

1. **Backend `plugin_host` is dead-as-of-2026-04-29** — confirmed. 1,283 LOC across three files (`manifest.rs` 165, `runtime.rs` 69, `validation.rs` 1056 + 7 mod.rs) with zero consumers in `rim_api`, `rim_infra_services`, or `rim_server`. Doctor-strange's "1,290" is accurate to within rounding.
2. **Capability registry is in production use** — confirmed. `CapabilityService::check` + `check_many` are wired through `DefaultCommandService::filter_ai_surface` (AI surface filter), `DefaultAuditService` (`audit/mod.rs:103`), and the dispatch path. Not a skeleton.
3. **H2 closure migration (`m20260426_200000_capability_registry_add_is_self_scoped`)** — confirmed clean. Adds `is_self_scoped BOOL` with safe default + targeted backfill of `audit.read.self`. Closes the `membership_holds(actor, None)` short-circuit cleanly. Vision's claim is accurate.
4. **Plugin-grant-store leg / where plugin state lives** — partial. The `capability/service.rs:23-25` docstring's "plugin-grant store lands in Phase 5" reference is now stale once Q1 is server-managed-marketplace-but-separate-service. **Per-user installed-plugin state belongs on the desktop client; signed catalog metadata lives on the marketplace service; `rim-server` does NOT grow plugin tables.** Argument in §1.4.
5. **`plugin_host/manifest.rs` has DTOs but no live consumers** — confirmed. Same evidence as claim 1.

**MCP verdict:** **Confirm falcon's estimate, contradict doctor-strange's "2-3 weeks."** Cheapest viable v1 (H1 + S1) is **4-5 weeks single-dev** assuming CLI v1 and AI agent reframe ship first. Both prerequisites are NOT yet shipped. Real-calendar floor with prerequisites included is 7-10 weeks. Doctor-strange's "2-3 weeks" is the optimistic floor *after prerequisites land*; treat as such. Scope-cuts available to fit the 6-12 month window in §2.4.

**Marketplace v1 scope estimate: M (4-6 weeks for one dev).** Just enough to catalog → sign-on-accept → desktop downloads-and-verifies one signed plugin from a hard-coded author allowlist. Author submission deferred to v1.5. CDN deferred to v1.5.

---

## Deliverable 1 — Vision's backend claims, validated

### 1.1 Claim: 1,290 LOC of dead `plugin_host` module

**Verdict: CONFIRMED.**

Files in `/home/kuba/RustroverProjects/rim-server/rim_services/src/plugin_host/`:

| File | Lines (rg counted) |
|---|---|
| `manifest.rs` | 165 |
| `runtime.rs` | 69 |
| `validation.rs` | 1056 |
| **Total (plus implicit `mod.rs` impact)** | **~1,290** |

Note: `rim_infra_services/src/plugin_host/` does **not exist** (Glob returned no matches). The 1,290 LOC lives entirely in `rim_services/src/plugin_host/` — the contracts crate.

**Consumer evidence (production wiring):**
- `rg "use\s+(rim_services|crate)::plugin_host"` returned **only intra-module imports** (i.e., `validation.rs` importing from `manifest.rs`).
- `rim_api/` has **zero** references to anything under `plugin_host`.
- `rim_infra_services/` has **zero** references to anything under `plugin_host`.
- The only external mention is `rim_services/src/command/sanitize.rs:158` and `command/sensitivity.rs:126`, both in **comments/docstrings** referencing `LocalPluginManifestCommandDto::as_command_definition`. The function exists; nothing calls it from outside its own module.

**Refinement vs doctor-strange's claim:** the module compiles (it's `pub mod plugin_host` in `rim_services/src/lib.rs:25`) and the unit tests run, so it's not "dead" in the sense of `unused_imports`. It's "production-dead" — defined contracts with zero callers above the contracts layer. `validation.rs`'s docstring (line 17) admits this: "*The install path (when it lands later in Phase 2+) calls [`validate_plugin_manifest`] as the **single** entry point*". That install path has not landed.

**Implication for vision's plan:** vision's Phase 3 (a) backend scope estimate ("L") is roughly correct *but* it should NOT assume the existing `plugin_host` module is reusable as-is. Per Q1 = separate marketplace, this module's destiny is one of three:

1. **Move to the marketplace service repo** (validators are reusable for catalog ingestion).
2. **Move to a shared `rim_plugin_contracts` crate** consumable by both the marketplace service and any future server-side plugin-aware feature.
3. **Delete** if the marketplace service uses different DTO shapes (likely — the marketplace's wire shape is HTTP/REST, not the local-host JSON-RPC shape these DTOs were designed for).

I lean **option 2 short-term, option 1 long-term**. The N1 capability-key validator and F2 install-ID-leak walker are valuable invariants regardless of where they live.

### 1.2 Claim: capability registry is in production, not skeleton

**Verdict: CONFIRMED.**

Evidence trail:

- `rim_services/src/capability/service.rs:42-71` — `CapabilityService` trait declares both `check` and `check_many` (vision called this out correctly).
- `rim_infra_services/src/capability.rs:147` — production `check` impl with two-leg gate (registry + holder), strict default-deny, `is_self_scoped`-aware membership check.
- `rim_infra_services/src/capability.rs:524` and `:564` — `check_many` impl with bulk semantics.
- `rim_infra_services/src/command/surface.rs:30-34` — `DefaultCommandService::filter_ai_surface` calls `check_many` to filter AI-callable commands by per-actor capability set.
- `rim_infra_services/src/audit/mod.rs:103, 119` — `DefaultAuditService::list_entries` calls `check` for `audit.read.self` and `audit.read` capability gating.
- `rim_infra_services/src/command/dispatch.rs` — capability check threads through the dispatch path.
- Live tests calling `expect_check()`: `capability_gate_test.rs`, `reframe_phase1_smoke.rs`, `chat_list_channels_agent_test.rs`, `p4_a4_metadata_flips_test.rs`, `p4_a5_new_builtins_test.rs`, `ai_surface_filter_test.rs`.

The registry is wired through every command path. Vision's characterization is correct.

### 1.3 Claim: H2 closure (`m20260426_200000_capability_registry_add_is_self_scoped`) is clean

**Verdict: CONFIRMED.**

Migration at `/home/kuba/RustroverProjects/rim-server/migration/src/m20260426_200000_capability_registry_add_is_self_scoped.rs`. What it does:

1. `ALTER TABLE capability_registry ADD COLUMN IF NOT EXISTS is_self_scoped BOOL NOT NULL DEFAULT false`. Idempotent. Conservative default (workspace-scoped → must supply context).
2. Targeted backfill: `UPDATE … SET is_self_scoped = true WHERE capability_key = 'audit.read.self'`.
3. Clean `down()` that drops the column.

**Why this is a real H2 closure:** prior to this migration, `membership_holds(actor, None)` short-circuited to `Ok(true)` whenever `workspace_id` was absent. A new builtin that legitimately required workspace membership but was dispatched without context would silently bypass the membership check — a latent privilege-escalation primitive. The new column makes "self-scoped" a per-row property of the registry, not a hardcoded function in the gate code. `CapabilityService::check` rejects `workspace_id = None` for non-self-scoped caps with `PermissionDenied` (per `capability.rs:147` + `:246` comment).

**Operational notes:**
- Backfill is idempotent (re-running the migration is safe).
- Down-migration is destructive (drops the column) but doesn't violate any invariant — `is_self_scoped = false` is the correct default for any new schema rev that adds the column back.
- Vision's plan §"Honest corrections vs. discovery memo" #1 is accurate.

**Small nit for vision to update:** the `capability/service.rs:23-25` docstring still references "plugin-grant store lands in Phase 5" as future work. That phrasing predates Q1 lock-in. Per §1.4 below, the phrasing should now read something like "for `Plugin`-source capabilities, the actor must hold an explicit grant — Phase 1 ships the workspace-membership half only. Plugin-source caps are deferred until either (a) a server-side plugin-host story exists [not in v1] or (b) the marketplace service grows a per-user plugin install endpoint that mirrors back into rim-server [also not in v1]." This is a comment update, not code; flagging for vision/strange to make the docstring accurate.

### 1.4 Claim: where plugin grants and per-user plugin state should live

**Verdict on vision's framing: PARTIAL.**

Vision wrote "the plugin-grant store leg is the missing piece for plugin work but only matters if Q1 is server-managed." Q1 is now answered: **separate marketplace service**, NOT part of `rim-server`. So the question becomes a three-way choice for per-user installed-plugin state:

| Candidate | Argument for | Argument against |
|---|---|---|
| **(A) Marketplace service DB** | Centralized catalog already lives there; supports cross-device "show me what I have installed" later. | Adds a second auth model (the marketplace service must accept rim-server-issued tokens or its own); cross-device sync is explicitly v2+ per vision. |
| **(B) `rim-server` DB** | Existing auth + audit log + capability registry is right here; one less moving part. | Pulls plugin awareness back into rim-server, which Jakub explicitly said NOT to do. Drags in plugin install lifecycle FK to capability_registry that we're trying to keep deferred. |
| **(C) Desktop-local** (current state) | Matches `plugin_model.md`: plugins are user-local, desktop-host owned. Already shipped: `~/.rim/plugins/<plugin_id>/` + `plugin_state.json`. Zero new infra. | Loses cross-device sync. User on a new laptop has zero plugins until they re-install. |

**Recommendation: (C) for v1, with a forward-compatible shape.**

Reasoning:
- Q1 was specifically about distribution (centralized catalog), NOT about installation state. The two are separable: the marketplace serves a `GET /plugins/<id>/manifest.json` and `GET /plugins/<id>/<version>/bundle.tar.gz`; the desktop verifies signature and installs locally. **Nothing about the centralized catalog requires server-side install tracking.**
- Cross-device plugin sync is explicitly out-of-scope per vision's plan ("v2+ per G12").
- The desktop already persists `PluginStateStore` to JSON in app_data_dir (per vision's discovery §1.2). That's the right home.
- Forward-compatibility hook: define the desktop-local install state shape NOW (`installed_plugins: [{plugin_id, version, source: "marketplace"|"sideload", marketplace_signature_id?, fingerprint?, installed_at, enabled, approved_capabilities}]`) so option (A) can later be a thin sync layer over the same shape.

**`rim-server`'s position:** unchanged from today. No `plugin_install` table, no `plugin_listing` table, no plugin GraphQL surface. The capability_registry's `plugin_id` FK column stays nullable and unused for the foreseeable future. The marketplace service is responsible for everything plugin-distribution-related; rim-server is responsible for everything chat/command/audit/capability-related; the desktop is responsible for runtime + install state. Three boundaries, each owns one thing.

### 1.5 Claim: `plugin_host/manifest.rs` has DTOs but no live consumers

**Verdict: CONFIRMED.** Same evidence as §1.1. The validation tests pass, but no code in `rim_api` or `rim_infra_services` calls `as_command_definition` or constructs a `LocalPluginManifestDto` outside the test fixtures inside `validation.rs` itself.

---

## Deliverable 2 — MCP feasibility validation

### 2.1 Cross-check vs current state

I read falcon's sketch end-to-end. His architectural claim ("RIM is accidentally MCP-shaped") is accurate against today's code:

| Falcon's claim (sketch §) | Current-state evidence | Verdict |
|---|---|---|
| §2.2 "MCP server is just a special kind of RIM plugin" — same JSON-RPC 2.0 over stdio shape | `rim-ui/src-tauri/src/plugin_host/wire.rs` is JSON-RPC 2.0 with length-prefixed framing (per vision discovery §1.2). MCP's stdio transport is line-delimited JSON-RPC 2.0 (per modelcontextprotocol.io spec). Compatible at the framing layer with **one adapter** — not identical wire bytes. | Mostly correct; minor framing-adapter cost (length-prefix vs newline-delimited) falcon underplays. |
| §2.3 Tool discovery → command registration: MCP tool → `CommandDefinitionDto` with `source_kind: MCP`, `execution_target: MCP_RPC`, capability `mcp.<server>` | `CommandDefinitionDto` (rim_services/src/command/dto.rs:77) currently has `source_kind: BuiltIn|Plugin|Integration|User` and `execution_target: Backend|DesktopLocal`. Adding `MCP` and `MCP_RPC` variants is a new-variant-only enum extension — non-breaking but requires DTO + GraphQL enum extension. | Accurate; small but real schema work. |
| §2.4 Execution path: `CommandService::execute_command` dispatches by `execution_target` | Confirmed at `rim_infra_services/src/command/dispatch.rs`. Adding a new arm for `MCP_RPC` is mechanical. | Accurate. |
| §2.5 Capability gate maps cleanly | Confirmed — `CapabilityService::check` is the gate. New caps `mcp.<server-name>` would seed via the same builtin-seeding path. | Accurate; one watch-out — caps are bounded length 4..=128, charset `[a-z0-9_.]`. MCP server names with hyphens (`filesystem-mcp`) need normalization. Trivial. |
| §2.6 MCP Apps maps to plugin slot model 1:1 | Vision's discovery §1.2 confirms iframe + postMessage bridge already shipped, with origin lock + capability gating. The mapping holds. | Accurate. |
| §3.4 Auth alignment with CLI v1 — reuses API token model | **CLI v1 has NOT shipped.** No `ApiToken` entity in `entity/`, no `api_token` migration, no `auth.create_api_token` mutation. The `cli-v1-design-2026-04-22.md` design exists but is sketch-stage. | Falcon's prerequisite is real and unmet. |
| §5 Phase total v1 = L–XL combined, ~6–8 weeks calendar with one Rust dev + one frontend dev; cheapest viable = H1+S1 ~4-5 weeks single dev | Estimates are reasonable given the work itemized: `rim_mcp_host` crate (subprocess lifecycle, MCP handshake, JSON-RPC framing adapter, schema translation), `mcp_safe` field on CommandDefinitionDto, new variants on two enums, dispatch arm, plus frontend `panel_ref` and adapter. | Confirmed within ±1 week. |

### 2.2 Prerequisite chain — confirmed unmet

Falcon's §5 names two prerequisites:

1. **CLI v1 ships first** → establishes API token model. **Status: NOT SHIPPED.** No `ApiToken` entity, no migration, no mutation. CLI v1 is its own dedicated session per Jakub.
2. **AI Agent reframe ships first** → establishes `ai_callable` field which is "likely the same field as `mcp_safe`". **Status: PARTIALLY SHIPPED.** Per `actual_state.md` §5: Reframe Phase 1 + Phase 4 are GREEN (sanitize_nl_text, CommandInitiator, ai_callable, sensitivity, capability gate). Phase 5 is RED with 6 BLOCKs on Skill Macros (Strange remediating). The ship gate `command.discover` (#10) is blocked behind #2/#4/#5/#9.

**Implication:** Falcon's "4-5 weeks single dev for cheapest viable" assumes both prerequisites are done. They're not. **Real-calendar floor including prerequisites:**

- CLI v1 (separate repo, its own session): ~M, falcon estimates ~2 weeks per `cli-v1-design-2026-04-22.md` §0.
- AI Agent reframe close-out (Phase 5 #4 + #10 + remediation): ~2-3 weeks per Tony's Category A guidance.
- MCP cheapest viable (H1 + S1): ~4-5 weeks single dev.

**Sequenced calendar floor: ~8-10 weeks**, parallelizable down to ~6-7 weeks if the CLI v1 dev and the agent-reframe-closeout dev are different people.

### 2.3 Verdict

**Falcon's estimate: CONFIRM (4-5 weeks for cheapest viable v1, single dev).**
**Doctor-strange's "2-3 weeks": CONTRADICT.**

Doctor-strange's number drops MCP Apps UI rendering (H2) and Streamable HTTP server transport (S2), AND assumes both prerequisites are done. Doctor-strange himself flagged the estimate as "needs an iron-man spike" in his memo §6. Treat 2-3 weeks as the optimistic floor *after prerequisites land*. The honest working estimate is **4-5 weeks single dev for the cheapest cut, after CLI v1 + Phase 5 close, with prerequisites adding another 4-5 weeks of calendar in front.**

Vision's plan already encodes this correctly in Phase 6 — her "M (3-4 weeks for the smallest-viable subset, single dev) to L (6-8 weeks for the full v1)" range is exactly right, and her sequencing ("cannot precede P1; CLI v1 and agent reframe should ship first") matches what I'm validating. **Vision's Phase 6 needs no estimate revision.**

### 2.4 Scope-cuts that fit the 6-12 month differentiation window

If the full prerequisite chain is too long, three orthogonal axes to cut:

**Cut A — Defer the server half (S1+S2) entirely.** Ship MCP host (H1) only — RIM consumes external MCP servers, but external AI clients can't drive RIM. Rationale: most strategic value of MCP-shaped RIM is the **ecosystem inflow** ("plugin marketplace explodes overnight" per falcon §1). Outflow (RIM as MCP server) is nice-to-have; the host half delivers 70% of the differentiator at ~50% of the work.

- **Estimate: ~3 weeks single dev** (just H1, sans CLI v1 prerequisite — host doesn't need API tokens, it spawns local subprocesses).
- **Strategic loss:** RIM is reachable from Cursor/Claude Desktop only via copy-paste, not as a tool target. Acceptable for v1; revisit when usage signal warrants.

**Cut B — Defer MCP Apps UI (H2).** Ship `tools/list` + `tools/call` dispatch but render results as plain text only, no iframe panels. Means RIM behaves like Claude Desktop's MCP integration (text-only) until MCP Apps is added.

- **Estimate impact:** drops ~1 week from H1+S1 v1.
- **Strategic loss:** loses the "AI panels for free" pitch falcon highlighted. Defensible because MCP Apps adoption in the wild is still nascent (Jan 2026 spec); few servers ship UI bundles yet.

**Cut C — Coarse capability per server only (no fine-grained tool prompts), document the "trust the install" posture explicitly.** This is what falcon already proposed for v1. Saves ~3 days vs designing fine-grained.

**Recommended cut combo for the 6-12 month window: A + C.** Ship MCP host stdio (H1), coarse capabilities, document the trust posture. Skip the server half until CLI v1 lands and the API-token model is real. ~3 weeks single dev, deliverable in parallel with CLI v1 work, no schema changes to `rim-server` beyond two enum-variant extensions on `CommandSourceKindDto` and `CommandExecutionTargetDto`.

---

## Deliverable 3 — Marketplace service discovery

Per Jakub's Q1 lock-in: **separate service, new repo, standalone deployment.** Discovery only — no spec, no code.

### 3.1 Repo / deployment shape

**Recommendation: new repo `rim-marketplace`, simpler stack than `rim-server`.**

Argument: the marketplace's workload is fundamentally different from `rim-server`'s.

| `rim-server` workload | `rim-marketplace` workload |
|---|---|
| Per-user, per-workspace, per-conversation read/write at high frequency. Real-time WS events. Multi-tenant. Strong consistency on commands/audit. | Mostly read: catalog browse, search, version lookup, bundle download. Multi-tenant only at the author level; users browse anonymously or with a desktop client identity. Cache-friendly. |
| Actix + async-graphql + SeaORM + Postgres + WebSocket + Stronghold. ~30+ migrations. | Catalog with ~100s of plugins growing to ~1000s. Read-heavy. Static-asset-heavy (bundles, icons, screenshots). |

**Stack proposal:**

- **Axum** (lighter than Actix; idiomatic for read-heavy REST APIs).
- **Plain REST** (NOT GraphQL). Marketplace endpoints are simple resource-style — `GET /plugins`, `GET /plugins/<id>`, `GET /plugins/<id>/versions/<v>/manifest`, `GET /plugins/<id>/versions/<v>/bundle`. GraphQL adds nothing here. Bonus: REST + ETag/Last-Modified caches well at CDN edge.
- **SQLite OR Postgres** — start with SQLite for v1 (read-heavy catalog, single instance is fine), promote to Postgres if/when author submission API and write traffic justify it. SQLite + WAL handles thousands of QPS read trivially. **Argument for SQLite:** zero ops cost, file backup, simpler dev. **Argument for Postgres:** consistency story across multiple marketplace replicas if we ever scale out. v1 ships single instance; SQLite is fine.
- **Bundle storage:** filesystem-on-host for v1 (single deploy), `S3-compatible blob` for v1.5. CDN-fronted is v1.5 (Cloudfront / Fastly with origin = the marketplace host).

**Workspace shape:** new top-level Rust crate at `~/RustroverProjects/rim-marketplace/`, NOT a member of the `rim-server` workspace. Same justification as falcon's CLI v1 reasoning (`cli-v1-design-2026-04-22.md` §0.1): the marketplace is a separate deployable, separate version, separate trust boundary, and forcing it through GraphQL or rim-server's auth model is an anti-coupling.

**What the marketplace MUST NOT depend on:** `rim_services`, `rim_infra_services`, `rim_infra_db`, `rim_api`. If anything is needed cross-repo, it goes through HTTP. Same discipline falcon applied to CLI v1.

**What the marketplace MAY share:** a tiny `rim_plugin_contracts` crate (published to crates.io or vendored) holding the manifest DTO shapes — same shapes the desktop already parses. Single source of truth for `PluginManifest`. This is option 2 from §1.1.

### 3.2 Data model (rough)

Tables (SQLite or Postgres-portable):

- `plugin` — `id` (UUID v7), `slug` (`dev.rim.hello-world`), `display_name`, `author_id`, `summary`, `description_markdown`, `created_at`, `updated_at`, `latest_version_id`, `download_count` (derived), `status` (`pending|approved|deprecated|removed`).
- `plugin_version` — `id`, `plugin_id`, `version` (semver string), `manifest_json` (full manifest), `bundle_blob_ref` (path/key), `bundle_sha256`, `published_at`, `marketplace_signature` (binary blob, marketplace-key signed), `author_signature` (binary, author-key signed; optional in v1 if author submission is deferred).
- `plugin_author` — `id`, `display_name`, `contact_email` (verified), `public_key` (Ed25519, hex-encoded), `key_added_at`, `revoked_at`.
- `plugin_signing_key` — marketplace's own signing key history. `id`, `public_key`, `created_at`, `revoked_at`. Latest active key is what desktop clients verify against.
- `plugin_install_count` (denormalized) — `plugin_id`, `daily_count`, `total_count`, `last_updated`. Async-rolled-up; desktop reports installs via a fire-and-forget POST.

**Bundle storage:** `bundle_blob_ref` is a logical key. v1 maps to filesystem path `/var/lib/rim-marketplace/bundles/<plugin_id>/<version>.tar.gz`. v1.5 maps to S3 key. Schema doesn't change.

**Per-user installed-plugin state — does the marketplace track this?**
**No, not in v1.** Per §1.4, that lives desktop-local. The marketplace tracks aggregate install counts (anonymous), not per-user identity. Adding per-user state means adding an auth model on the marketplace side — defer to v2 + cross-device-sync work.

**Ratings / reviews / install counts:**
- **Install counts: v1.** Anonymous, aggregate. Worth ~2 days of work; useful for the "popular plugins" list day one.
- **Ratings + reviews: v1.5 or v2.** Requires user identity on the marketplace, moderation pipeline, abuse handling. Out of scope for "ship one signed plugin" v1.

### 3.3 Signing infrastructure

**Trust chain (v1):**

```
Plugin author key (Ed25519)
    ↓ signs bundle.tar.gz on submit
[Author signature]
    ↓ marketplace verifies on accept
Marketplace signing key (Ed25519, single active key)
    ↓ co-signs bundle.tar.gz on accept
[Marketplace signature] (the one shipped with the bundle)
    ↓ desktop client verifies on install
Desktop trust = signature valid + signed by current-or-recent marketplace key
```

**Format choice: cosign-style detached signatures.**

Argument:
- **Don't sign manifests inline.** Keeping the signature alongside the bundle (sidecar `.sig` file or HTTP header) means the manifest stays human-readable JSON; a curl-and-eyeball sanity check still works. Inline signatures (manifest with embedded `signature` field) need re-canonicalizing the JSON before verifying — a class of bugs we don't need (cf. JSON canonicalization CVEs in the wild).
- **cosign-style** (or its primitive `signify`/`minisign`): detached `.sig` file containing `(scheme, key_id, signature_bytes)` over `SHA256(bundle.tar.gz)`. Simple format. Easy to verify with a 50-line Rust function or a shell script.

**Key rotation, revocation:**
- Desktop client ships with a hardcoded list of marketplace public keys (current + last-1). Adding a key requires a desktop release. **Acceptable for v1** — the marketplace key rotates on the order of years.
- Author key revocation: marketplace stores `revoked_at` on `plugin_author.public_key`. Bundles signed before revocation stay valid; bundles signed after are rejected on submit. Already-installed plugins on user machines are NOT revoked retroactively in v1 (no signal channel — adding one is a v1.5 task, e.g., a `GET /revoked-keys.json` the desktop polls weekly).
- **Don't over-engineer for v1.** No transparency log, no Sigstore-style identity-bound certs, no `n-of-m` signing. One marketplace key, one author key per author, simple revocation flag. Pull in heavier infra when warranted by attacker model.

### 3.4 Public API surface (rough)

REST, all responses JSON unless stated:

- `GET /v1/plugins` — list/search. Query params: `q` (full-text), `tag`, `sort` (popular|recent|name), `limit`, `cursor`. Returns paginated array of plugin summaries.
- `GET /v1/plugins/<plugin_slug>` — plugin details, latest manifest, version history (last 10), screenshots, signing key fingerprints.
- `GET /v1/plugins/<plugin_slug>/versions/<version>/manifest` — manifest JSON for a specific version.
- `GET /v1/plugins/<plugin_slug>/versions/<version>/bundle` — bundle download. Streams `application/x-tar+gzip`. Returns `X-Marketplace-Signature` and `X-Marketplace-Key-Id` headers (or, equivalently, redirects to a signed CDN URL with the same metadata in headers).
- `GET /v1/keys` — list of currently-active marketplace signing public keys (for desktop verification).
- `POST /v1/plugins/<plugin_slug>/install-event` — fire-and-forget install counter bump. Body: `{ desktop_install_id_hash, version }`. Anonymous; rate-limited per IP.
- `GET /v1/health` — operator endpoint.

**Author submission API — out of scope for v1.** v1 ships a hard-coded set of plugins (likely 3-5: hello-world, markdown-preview, timestamp-snippet, plus 1-2 demo plugins), submitted manually by the RIM team via a CLI script that the marketplace operator runs against a private admin endpoint. **Author submission is v1.5** — that's where we'd add `POST /v1/plugins` with author auth, the moderation pipeline, etc.

### 3.5 Trust path — registry-signed AND TOFU sideload

Per Jakub's Q4: both lanes shipped. They share a common desktop install lifecycle but diverge at the verification step.

**Common (both lanes):**
1. User initiates install (from marketplace UI or git URL or local folder).
2. Desktop downloads bundle → stages in `~/.rim/staging/`.
3. Desktop validates manifest (existing `plugin_host/manifest.rs` validation).
4. **Trust step** (lane-dependent — see below).
5. If trusted, desktop runs capability prompt (existing `PluginCapabilityPrompt`).
6. Approved → bundle moves to `~/.rim/plugins/<plugin_id>/`, `plugin_state.json` records install.

**Lane A — registry-signed:**
- Bundle arrives with marketplace signature in `X-Marketplace-Signature` header (or sidecar `.sig` file).
- Desktop verifies signature against locally-stored marketplace public key list.
- Sig invalid → install rejected with "marketplace signature invalid; do not trust this bundle." No fallback.
- Sig valid → record `source: "marketplace"` and `marketplace_signature_id: <sig_hash>` in state.

**Lane B — TOFU sideload:**
- Bundle arrives via git clone or local-folder install. No marketplace signature.
- Desktop computes SHA256 of bundle → records as fingerprint in state.
- First install: warn user "this plugin is not from the marketplace; trust on first install. Bundle fingerprint: SHA256..."  Prompt confirms. Record `source: "sideload"` and `fingerprint: <sha256>` in state.
- Subsequent updates from the same source: if fingerprint changes (e.g., `git pull` produced new content), warn user "this plugin's content has changed since first install; review before continuing."  Prompt to accept new fingerprint or abort.
- This is exactly the SSH known_hosts model. No new crypto, just SHA256 + a small per-plugin fingerprint store.

**Shared install lifecycle in the desktop host?** Yes. The trust step is a typed enum:

```
TrustOutcome {
    MarketplaceSigned { sig_id, key_id },
    TofuFirstInstall { fingerprint },
    TofuFingerprintMatch { fingerprint },
    TofuFingerprintChanged { old, new, accepted_by_user },
    TrustRejected { reason },
}
```

Everything downstream of the trust step is identical: capability prompt, install, enable. The trust outcome variant gets recorded in `plugin_state.json` for the trust badge UI to read.

### 3.6 What `rim-server` does NOT do (per Jakub's call)

**Confirmed implications:**

- **No plugin tables.** No `plugin_listing`, no `plugin_version`, no `plugin_install` migrations land on `rim-server`. Vision's Phase 3 (a) backend scope drops to **~zero** — the work moves to `rim-marketplace`.
- **No GraphQL plugin surface.** No `plugins(filter)` query, no `installPlugin` mutation, no `publishPlugin` admin mutation on rim-server. If a desktop wants to display "what plugins are available," it talks to `rim-marketplace` directly (HTTP/REST), not through `rim-server`.
- **Capability registry's `plugin_id` FK column stays unused for v1.** The deferred FK constraint mentioned in `m20260424_120000_create_capability_registry.rs:13-15` does NOT need to land in v1 — there's no `plugin_install` table on `rim-server` to FK against. The column stays nullable, the existence-check stays a code-level invariant. **Vision's Phase 3 backend gap analysis row "Capability registry FK: lands with plugin_install" is OBSOLETE under Q1 = separate marketplace.** Surface this for vision to update.
- **No plugin-grant store on rim-server.** Per §1.4, plugin-grant is desktop-local (or marketplace-side in v2). The `capability/service.rs:23-25` docstring's "Phase 5" reference is stale and should be rewritten as "deferred indefinitely; Plugin-source capabilities deny everywhere on the server, which is correct because plugins don't run on the server." Cosmetic, but worth fixing.
- **Plugin command IDs (`plugin.<id>.<verb>`) stay opaque strings on `rim-server`.** That's already true. No change.
- **The existing `rim_services::plugin_host` module** (1,290 LOC of dead DTOs/validators) — should be moved to `rim_plugin_contracts` shared crate (option 2 from §1.1) so both desktop and marketplace can consume the same N1/F2 validators. Don't delete — the validators are valuable. Don't keep in `rim_services` — they're not server contracts.

### 3.7 Marketplace v1 scope estimate

**v1 = "ship one signed plugin from the catalog, end-to-end, where the catalog is hard-coded by the RIM team and the desktop verifies signatures."**

Scope breakdown (one dev, four-week sprint as a rough unit):

| Sub-phase | What | Estimate |
|---|---|---|
| MVP-1 | Repo skeleton (`rim-marketplace`) + Axum + SQLite + read endpoints (`GET /plugins`, `GET /plugins/<slug>`, `GET /plugins/<slug>/versions/<v>/manifest`, `GET /plugins/<slug>/versions/<v>/bundle`) + 5 hard-coded plugins seeded via SQL fixtures | 1 week |
| MVP-2 | Marketplace key generation + bundle co-signing on accept (admin-only CLI tool — not an API yet) + `GET /v1/keys` endpoint | 0.5 week |
| MVP-3 | Desktop signature verification on install (Ed25519 verify against shipped key list) + `TrustOutcome::MarketplaceSigned` recorded | 0.5 week (desktop-side) |
| MVP-4 | Frontend marketplace browser (vision's Phase 3 work — calling `GET /v1/plugins`) + TanStack Query hook | 1 week (vision/spider-man) |
| MVP-5 | TOFU sideload lane wired in parallel + fingerprint storage + on-change warning UI | 0.5 week (desktop) |
| MVP-6 | Install count fire-and-forget endpoint + telemetry plumbing + popular-list ordering | 0.5 week |
| MVP-7 | Operator deploy story (Dockerfile, simple compose, healthcheck, certs) + ADR + README | 0.5 week |

**Total: ~4-5 weeks for one dev** focused exclusively on the marketplace, of which ~2-3 weeks is rim-marketplace-repo work and ~2 weeks is desktop-side verification + frontend browser. **Estimate: M.**

**Out of v1 (deferred to v1.5+):**
- Author submission API + auth + moderation pipeline (~2-3 weeks).
- Ratings, reviews (~2 weeks + moderation overhead).
- CDN fronting + S3 bundle storage (~1 week).
- Admin web UI (~2 weeks).
- Cross-device install sync (v2+).
- Per-user installed-plugin tracking on the marketplace (v2+ — pulls in auth boundary).
- Key rotation tooling beyond the bundled-key-list approach (v1.5+).
- Revocation channel for already-installed plugins (v1.5).

---

## Things in vision's plan that need revision based on these findings

Surface clearly so fury can have her update:

1. **Vision's "Backend (rim-server) gap analysis" §"Per Q1 answer"** — the (a) row's "Iron-man scope estimate (provisional): L" is now WRONG. With Q1 = separate marketplace service, `rim-server`'s plugin scope is **none** for v1, not L. The L estimate moves to a new `rim-marketplace` repo and counts as separate scope, not `rim-server` scope.

2. **Vision's "Capability registry — natural next step"** — the recommendation that "if Q1=a/c, implement plugin-grant store + the FK constraint that's deferred. **S–M backend.**" is OBSOLETE under Q1 = separate marketplace. Plugin-grant store does NOT need to land on `rim-server` for v1 (or v1.5 — it's a v2 question tied to cross-device sync). Vision should update this row to "Plugin-grant store on rim-server: deferred indefinitely. Plugin-source capabilities deny everywhere on the server, which is correct."

3. **Vision's Phase 3 (a) branch — "NEW migrations (plugin_listing, plugin_version, plugin_install), NEW `rim_services::plugin_marketplace` module, NEW GraphQL query/mutation in `rim_api`. Storage: bundle CDN or Postgres large-object."** All of this lives on `rim-marketplace`, NOT `rim-server`. The plan should be updated to say "Phase 3 (a): backend scope on `rim-server` = none. Backend scope on new `rim-marketplace` repo = M (4-5 weeks). See iron-man's marketplace discovery memo for shape."

4. **Vision's Phase 4 (a) — "If Q1=a: New trust-authority concept: signing key registry. Probably reuses the existing capability registry pattern (table + service trait)."** Wrong location. Signing key registry lives on `rim-marketplace`, not `rim-server`. Capability registry pattern is a fine reference, but it's a new table on a new repo. Backend scope on `rim-server` for Phase 4 (a) is **none**. (Phase 4 still has frontend scope for `signatureVerifier.ts` + `TrustBadge` wiring.)

5. **Vision's "Areas where I'd most want iron-man pushback" #3** — "Is the existing `m20260426_200000_capability_registry_add_is_self_scoped` migration actually solid for the plugin grant work, or does it need another extension?" Answer: it's solid for the H2 closure it shipped. Plugin grant work is no longer landing on `rim-server` (per Q1), so the question is moot. No further migration extension needed.

6. **Vision's reading of the `capability/service.rs:23-25` docstring's "Phase 5" reference** — vision correctly notes the Phase 5 reference is rim-server-internal phase numbering, not vision's plan numbering. After Q1 lock-in, the docstring needs a comment-only update to remove the dead "Phase 5" reference and replace with "Plugin-source capabilities deny on the server unconditionally; plugin execution lives on the desktop." Trivial change but worth doing in the same PR that touches that file next.

7. **MCP feasibility — vision was right.** Phase 6 estimate (M for cheapest cut to L for full v1) and prerequisite chain (CLI v1 + Phase 5 close before MCP) need NO revision. Confirmed.

---

## Open questions for jakub (only if he wants to know)

Not blockers — these are second-order calls iron-man would prefer jakub make rather than invent.

- **Is `rim_plugin_contracts` shared crate worth publishing to crates.io?** Or is vendoring it across two repos fine? (Vendor is simpler for one team, fewer publish gates; crates.io is cleaner long-term.)
- **Marketplace key custody.** Where does the marketplace signing key live in production? Hardware token (YubiHSM) is overkill for v1; OS-keychain on a build host is fine. Make it a v1 ops decision, not a v0 design decision.
- **Marketplace-as-shared-infra-vs-self-hosted-vs-both?** v1 = single RIM-team-operated marketplace. Some enterprise customers will eventually want self-hosted. v1 ships single; v2 considers self-hosted-with-its-own-key. Note for the roadmap.

---

— iron-man
