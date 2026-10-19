> **Status:** Implemented 2026-04-28, hardened 2026-04-28. See commits `9538c66`, `c1e3846`.
> **Do not edit** — historical record. For changes, write a new ADR.

# Registration Redesign — Backend Spec

**Author:** the backend architect
**Date:** 2026-04-28
**Status:** Spec only. No code in this doc. The implementer builds it; the reviewer reviews; the coordinator merges.
**Audience:** the implementer, the frontend lead (parallel frontend spec), the security reviewer (security touchpoints flagged), the coordinator (gatekeeper).

**REVISIONS (2026-04-28, post-impl rulings from the architect):**
- **R1 — Test 10 vs Test 12 contradiction:** RESOLVED in favor of Test 12 (default-7-days). Test 10 dropped. See §6.2 and §8.1.
- **R2 — Rate limiter wiring:** RESOLVED option (a) — accept the implementer's internal-construction design. §8.3 boot-fail tests for rate limiters are dropped. Cosmetic `let _ = &..._rate_limiter;` lines in `app.rs:264-282` to be removed by the implementer in a follow-up cleanup. See §4.7, §6.3, §8.3.
- **R3 — Resolver tests 13–15 placement:** RESOLVED — placed in `rim_api/tests/registration_resolvers_e2e.rs` under the `e2e-tests` feature, requiring `DATABASE_URL`. Unit-test variants already covered in §8.1 (Tests 20–25) via `rim_api/src/tests.rs` schema-only harness. See §8.2.
- **A1 — Amendment 2026-04-28b:** post-impl product gap closure. Adds required `email` and optional `displayName` to `SignupWithInviteInput`. Migration adds `user.email` (NOT NULL once backfilled) with case-insensitive unique index; `user.display_name` already exists as nullable on the entity (no migration needed for that column). New `ServiceError::EmailTaken` variant + `EMAIL_TAKEN` GraphQL extension code. Test floor moves from 33 to **40**. See §13.

---

## 0. Executive Summary

RIM is multi-tenant and **invite-only by default**. Self-service signup is opt-in via a single env flag. The first user on a fresh deployment is bootstrapped through a CLI command on `rim_server` that prints a single-use, short-TTL signed link; every subsequent account is created through an invite token (workspace invite, already shipped, or a new **instance invite** issued by an admin). The login page is for existing users only.

The product decision is fixed. This doc specifies:

1. The **`instance_invite`** table (new) and why it is separate from `invite`.
2. The **`bootstrap-user` CLI subcommand** on `rim_server`, plus its in-DB token row with consumption + revocation.
3. The new **`signupWithInvite`** GraphQL mutation, gated by a `RegistrationMode` resolved at boot time from `RIM_OPEN_REGISTRATION`.
4. **Three security flags** from the product memo, addressed inline (TTL, revocation, fail-closed startup wiring with `tracing::warn!` + rate-limit precondition).
5. Test list (40 tests, post-A1 floor), migration sketch with rollback note, GraphQL SDL diff.

The scope here is **registration only**. Email-domain allowlist, OIDC/SAML JIT, and plugin marketplace registration are explicitly out of scope (see §11).

---

## 1. Glossary

| Term | Meaning |
|---|---|
| **Bootstrap link** | One-shot signed URL emitted to stdout by `cargo run -p rim_server -- bootstrap-user`. Carries a `bootstrap_token`. |
| **Instance invite** | Server-admin-issued invite that lets a stranger create an *account* on this RIM instance. New concept. |
| **Workspace invite** | Existing `invite` row issued by a workspace owner that lets the holder *join a workspace*, optionally creating an account in the same flow. Unchanged. |
| **Invite-only mode** | Default. Signup mutation requires a valid `instance_invite_token` OR `workspace_invite_token`. |
| **Open mode** | `RIM_OPEN_REGISTRATION=true`. Signup mutation accepts no token; rate-limited. |
| **`Actor`** | `rim_services::Actor` — authenticated principal + `UserId`. |

---

## 2. Token Model — call: separate `instance_invite` table

### 2.1 Decision

**Add a new `instance_invite` table.** Do **not** extend the existing `invite` table with a row variant.

### 2.2 Why

The existing `invite` table (created in `migration/src/m20260212_000001_create_sessions_and_invites.rs`) has three properties that bake in workspace semantics:

- `server_id` is `NOT NULL` and FK-restricted to `Server::Id`.
- `created_by_user_id` FK-restricts to `User::Id` (`ON DELETE Restrict`).
- Consumption (`try_consume`) increments `use_count` and the calling code in `accept_invite` adds a `ServerMember` row of role `Guest` against `inv.server_id`.

An instance invite has none of those:

- Not bound to any workspace; the new user lands in "no workspaces yet" state.
- Issuing identity for a bootstrap invite is the **server operator**, who has no `User` row yet on a cold start. Forcing `created_by_user_id NOT NULL` would block cold-start bootstrap.
- Consumption produces a `User`, never a `ServerMember`.

A discriminator column on `invite` (`kind: 'workspace' | 'instance'`) means every existing `accept_invite` path branches at the top, every repo trait gains conditional FK semantics, and the schema constraint that `server_id` is required for the workspace variant becomes a CHECK constraint instead of a NOT NULL — strictly worse for query plans and harder to read at the schema level.

Two tables, one trait per table, zero shared columns that pretend to be the same thing. Existing `Invite`/`InviteRepository` stays as-is.

### 2.3 Naming note

To avoid confusion with the existing `Invite` Rust type, the new domain types are named `InstanceInvite` (table) and `BootstrapToken` (table — see §3). No reuse of the bare word `Invite`.

---

## 3. Bootstrap CLI Command

### 3.1 Surface

```
cargo run -p rim_server -- bootstrap-user --email <email>
```

**Flag:** `--email <email>` (required). Used as the destination identifier baked into the token; the eventual signup form pre-fills it but does not require it match (see §3.5 — email is informational, the token is the credential).

**Behavior:**

1. Connects to the configured DB using `RIMConfig::load()` — same env reading as the server entrypoint.
2. Inserts one `bootstrap_token` row.
3. Prints to **stdout** a single line: `https://<host>/join/<raw_token>` (path-param form, matching the rim-ui `/join/:token` route — verified in `rim_server/src/bootstrap_cmd.rs:91`) where `<host>` defaults to `${RIM_SERVER_HOST}:${RIM_SERVER_PORT}` and is overridable with `--public-url <url>`.
4. Prints to **stderr** the token id, expiry, and SHA-256 fingerprint (for audit log correlation). Does **not** print `<raw_token>` to stderr.
5. Exits 0 on success; nonzero on DB error or entropy failure.

**Runnable cold (zero users) AND warm (operator self-service):** Yes. Cold-start path does not require an `Actor`. Warm-start path also does not require an `Actor` — the CLI has shell access, which is the trust boundary. There is no in-app "instance admin" role in this phase (see §5).

### 3.2 Token storage — call: in-DB row

**Decision: in-DB row, not stateless JWT.**

**Why:** JWT-style tokens are stateless and cannot be revoked, audited, or single-use-enforced without a separate revocation list, which is just a worse in-DB row. We want all three: revocation (so a leaked link can be killed), audit (so we know who consumed which token and when), and single-use (consumption mutates state). All three require server state. Use the same `(token_hash, expires_at, used_at)` shape that `password_reset_token` already uses (see `migration/src/m20260404_000000_create_password_reset_token.rs`).

The raw token is generated with the same primitive as session/invite tokens (`rim_auth::session_token::generate_session_token`, prefixed `rim_boot_`), hashed with SHA-256, only the hash is stored. The raw token is shown to stdout exactly once and never persisted. Lookup at consumption time is by `token_hash`.

### 3.3 Signing scheme

There is no separate HMAC layer over `(token_id, email, exp)`. The token itself is a 256-bit cryptographic random value (same primitive as session tokens — see `rim_auth/src/session_token.rs:6`). Authenticity is established by the lookup: if a SHA-256 hash of the presented raw token matches a non-consumed, non-revoked, non-expired row, it's valid. No HMAC key needed.

**Rationale:** HMAC-signed bearer tokens (JWT-style) over a random nonce add zero security beyond random-token-plus-DB-lookup *as long as the random token has sufficient entropy*. 256 bits does. Adding a key derivation also creates a new secret to manage and rotate. The simpler design wins; this matches the existing session-token pattern.

### 3.4 TTL — **15 minutes (security flag #1)**

`expires_at = now() + 15min`. This is per the product memo recommendation. Hardcoded; not user-configurable from the CLI (an operator who needs longer can re-issue). Configurable via `RIM_BOOTSTRAP_TOKEN_TTL_SECONDS` env var with a hard upper bound of 3600 (1 hour) enforced in `RIMConfig::load`. Values above the cap fail-closed at config load.

### 3.5 Consumption semantics

The `signupWithInvite` mutation (§4) accepts a `bootstrapToken: String` *or* an `instanceInviteToken: String` *or* a `workspaceInviteToken: String` (mutually exclusive — exactly one must be present in invite-only mode; zero in open mode). On consumption of a `bootstrap_token`:

1. Look up by `token_hash`.
2. If not found / `used_at IS NOT NULL` / `revoked_at IS NOT NULL` / `expires_at < now()` → return one of the four errors per §4.5.
3. Inside a transaction: create the `User`, set `bootstrap_token.used_at = now()`, set `bootstrap_token.consumed_by_user_id = user.id`, create a session, commit. If commit fails, roll back — token is still consumable.
4. Audit log emits at `target = "audit"` with `bootstrap_token_id` and `consumed_by_user_id`.

The created user has no `ServerMember` row. They land in the "no workspaces yet" state described in §5.

---

## 4. Signup Mutation Gating

### 4.1 New mutation

`signupWithInvite(input: SignupWithInviteInput!): SignupWithInvitePayload!`

Replaces no existing mutation; coexists with the existing `acceptInvite` mutation. The existing `acceptInvite` retains its current behavior (workspace-scoped: account + workspace membership in one call, used for shared-link workspace invites). `signupWithInvite` is the new top-level, optionally workspace-less path.

**Why a new mutation rather than overloading `acceptInvite`:**

- `acceptInvite` returns `AcceptInvitePayload { userId, serverId }` with `serverId` non-null. Bootstrap and instance-invite signups have no server. Making `serverId` nullable on the existing payload is a public-API break.
- Three token types (bootstrap, instance, workspace) is enough divergence in error handling and downstream effects (server membership vs. none) that one mutation per concern reads cleaner.
- The frontend is wiring a single `/join/:token` page (path-param form) that calls `previewInviteToken` to discover the `tokenType` (`BOOTSTRAP` / `INSTANCE` / `WORKSPACE`) and routes the raw token to the matching input field on `signupWithInvite`; one mutation taking a sum-typed input is the cleanest server-side contract for that.

### 4.2 Resolution of registration mode

At server boot in `rim_server::app::build_app_runtime`, the env var `RIM_OPEN_REGISTRATION` is read and parsed into a `RegistrationMode` enum (`InviteOnly | Open`). The mode is wrapped in an `Arc<RegistrationMode>` and injected as GraphQL context data (alongside `Arc<dyn AuthService>`). The mutation reads it from `ctx.data::<Arc<RegistrationMode>>()`.

Boot validation (security flag #3 — see §6) runs in `build_app_runtime` *before* the schema is built; if validation fails, return `std::io::Error` from `build_app_runtime` so `main.rs` exits nonzero with the error logged at `target = "audit"`.

### 4.3 Input shape

> **See Amendment 2026-04-28b (§13)** — `email` (required) and `displayName` (optional) are added to this input. The block below is the original shape; the amended shape is in §13.2.

```rust
// rim_api/src/mutation.rs (new)
#[derive(InputObject)]
pub struct SignupWithInviteInput {
    pub username: String,
    pub password: String,
    /// Exactly one of these MUST be set in invite-only mode; all three MUST be
    /// absent in open mode. The mutation enforces this with a Validation error.
    pub bootstrap_token: Option<String>,
    pub instance_invite_token: Option<String>,
    pub workspace_invite_token: Option<String>,
}

#[derive(SimpleObject)]
pub struct SignupWithInvitePayload {
    pub user_id: ID,
    /// Some(_) only when `workspaceInviteToken` was used. Bootstrap and instance
    /// invites do not auto-attach a workspace.
    pub server_id: Option<ID>,
}
```

### 4.4 Algorithm

Pseudocode for the resolver:

```text
mode = ctx.data::<Arc<RegistrationMode>>()
count = bool(bootstrap_token) + bool(instance_invite_token) + bool(workspace_invite_token)

match mode:
  InviteOnly:
    if count != 1: return Validation("exactly one token required")
    dispatch on which one is set:
      bootstrap_token       -> auth.signup_with_bootstrap(cmd)
      instance_invite_token -> auth.signup_with_instance_invite(cmd)
      workspace_invite_token-> auth.accept_invite(cmd)  // existing path
  Open:
    if count > 0: return Validation("tokens must not be supplied in open mode")
    rate_limiter.check_and_record(remote_ip, "signup", LIMIT, now())
      -> if RateLimited: return ServiceError::RateLimited { retry_after_seconds }
    auth.signup_open(cmd)
```

Three new `AuthService` methods are added: `signup_with_bootstrap(cmd)`, `signup_with_instance_invite(cmd)`, `signup_open(cmd)`. Each returns a new `SignupResultDto` (mirrors `LoginResultDto` — `user_id`, `session`, `session_token`, `csrf_token`, plus `Option<ServerId>` always `None` for bootstrap/instance/open paths). The existing `accept_invite` keeps its current shape and continues to set the workspace.

### 4.5 Error mapping — distinguishable expired, indistinguishable consumed/invalid

Per the memo: do not let the FE distinguish *consumed* from *invalid* (that would let an attacker probe whether a leaked token was already used). *Expired* is OK to distinguish since it's not exploitable.

| Condition | `ServiceError` | GraphQL extension `code` |
|---|---|---|
| Token string not found in DB | `PermissionDenied` | `INVALID_TOKEN` |
| Token row exists but `used_at IS NOT NULL` | `PermissionDenied` | `INVALID_TOKEN` |
| Token row exists but `revoked_at IS NOT NULL` | `PermissionDenied` | `INVALID_TOKEN` |
| Token row exists but `expires_at < now()` | `PermissionDenied` | `EXPIRED_TOKEN` |
| Username already taken | `Conflict("username already exists")` | `CONFLICT` |
| Email already taken (see Amendment 2026-04-28b §13) | `EmailTaken` | `EMAIL_TAKEN` |
| Username/password/email/displayName validation | `Validation(detail)` | `VALIDATION` |
| Open-mode signup over rate limit | `RateLimited { retry_after_seconds }` | `RATE_LIMITED` |
| `RegistrationMode::InviteOnly` and zero/multiple tokens | `Validation(detail)` | `VALIDATION` |
| `RegistrationMode::Open` and any token present | `Validation(detail)` | `VALIDATION` |

Two new error codes, `INVALID_TOKEN` and `EXPIRED_TOKEN`, are added to `rim_api::errors::map_service_error`. They are produced by inspecting the `ServiceError::PermissionDenied` *plus* an optional discriminator on a new `ServiceError::ExpiredToken` variant, OR by adding two dedicated variants. **Decision: add two new `ServiceError` variants** — `InvalidToken` and `ExpiredToken` — and map them in `map_service_error`. Reusing `PermissionDenied` would force string sniffing at the API boundary, which violates the "no stringly-typed errors in new code" rule.

### 4.6 Error code "expired vs invalid" leak risk — re-checked

`EXPIRED_TOKEN` reveals that *some* row exists with this hash. An attacker who possesses a token but isn't sure if it was issued at all learns "it was issued, but you missed your window." That is by design — expiry isn't exploitable, the token is dead either way. We accept this leak.

### 4.7 Preview query — `previewInviteToken`

Per the frontend spec, the FE needs to inspect a token *before* rendering the signup form so it can (a) bail out early on `EXPIRED_TOKEN` / `INVALID_TOKEN`, (b) show "you're joining workspace X" copy on workspace invites, and (c) show generic "create your RIM account" copy on instance invites.

**New top-level query:**

```text
previewInviteToken(token: String!): InviteTokenPreview!
```

**Auth:** No `Actor` required. The token itself is the read credential — same logic as `signupWithInvite`. Resolver uses `ctx.data::<Arc<dyn AuthService>>()` directly without an `Actor` extractor.

**Read-only — does NOT consume.** The resolver only reads. It does **not** mutate `used_at`, `use_count`, or any audit field on `bootstrap_token` / `instance_invite` / `invite`. Confirmed: previewing is **not** consumption. No audit log row is written for a successful preview (a noisy log entry per FE page-load is worse signal than no log; admins still see the eventual `signupWithInvite` audit row, which is the actually-meaningful event). Failed previews (rate-limit hits) **do** log at `target = "audit"`.

**Token-type discrimination:** the resolver tries `bootstrap_token`, `instance_invite`, then `invite` (workspace) by hash, in that order. First non-revoked match wins. The three sources surface as **three distinct** `InviteTokenType` values to the FE so the FE can populate the correct GraphQL input field on `signupWithInvite`:

- `BOOTSTRAP` — token from the `bootstrap_token` table; FE routes the raw token to the `bootstrapToken` input field on `signupWithInvite`.
- `INSTANCE` — token from the `instance_invite` table; FE routes to `instanceInviteToken`.
- `WORKSPACE` — token from the `invite` (workspace) table; FE routes to `workspaceInviteToken`.

Verified against the live impl: `InviteTokenTypeGql` in `rim_api/src/types.rs` (three variants `Bootstrap`/`Instance`/`Workspace`) and the `preview_invite_token` mapping in `rim_infra_services/src/auth.rs` (lines ~1353–1443).

Note (FE cross-reference): the form copy ("Create your RIM account") is shared between `BOOTSTRAP` and `INSTANCE` on the FE — only the GraphQL input field differs. That UX detail is owned by the FE spec; documented here only so the contract on this side reads correctly.

**Workspace invite preview content:** workspace name (yes — already implied by the join URL the inviter shared, no new disclosure) and inviter display name (yes — the inviter consented to being named when they issued the invite; this is the same identity already shown in the existing `/invite/:token` flow). No privacy concern; we already disclose both in the existing `acceptInvite` UX.

**Instance invite preview content:** `tokenType: INSTANCE` and `expiresAt`. Nothing else. Specifically: no inviter name, no `max_uses`/`use_count`. The first prevents reverse-mapping a leaked token to an admin; the second prevents revealing how many seats remain on a multi-use invite.

**Bootstrap token preview content:** `tokenType: BOOTSTRAP` and `expiresAt`. Same minimal shape as instance invites. The FE renders identical "Create your RIM account" copy for both `BOOTSTRAP` and `INSTANCE`; the discriminator exists so the FE knows which `signupWithInvite` input field carries the raw token.

**Error mapping** (matches §4.5 — same indistinguishability rules):

| Condition | `ServiceError` | `extensions.code` |
|---|---|---|
| Token not found / consumed (single-use exhausted) / revoked | `InvalidToken` | `INVALID_TOKEN` |
| Token exists but `expires_at < now()` | `ExpiredToken` | `EXPIRED_TOKEN` |
| Rate limit exceeded for caller IP | `RateLimited { retry_after_seconds }` | `RATE_LIMITED` |

**Rate limit — chosen value:** **30 requests per IP per 5-minute sliding window**, keyed on `("preview_invite", remote_ip)`. Reuses the existing `rim_infra_services::command::rate_limiter::DashMap`-based limiter. Rationale:

- A legitimate user lands on `/join/:token` and triggers exactly one preview. Even with a hot-reload dev loop or a flaky network retrying, 30 in 5min is generous.
- A token-existence enumeration attack at 256 bits of entropy is not feasible regardless, but rate-limiting still matters for DB-load DoS — 30/5min/IP caps a single attacker at 8,640 lookups/day, which is cheap to absorb and trivially detectable in audit logs if sustained.
- Tighter than the open-mode signup limit (which is per-account-creation, more expensive); looser than auth-mutation limits (which gate password attempts).

**~~The limiter MUST be wired into GraphQL context regardless of `RegistrationMode` ... Test: `boot_fails_when_preview_rate_limiter_missing`.~~**

**RESOLVED (R2): Option (a) — accept internal construction.** The rate limiter is constructed inside `DefaultAuthService::new()` via `RegistrationRateLimiter::preview_default()`. It is unconditionally present; "missing limiter" is a state the type system makes unrepresentable. There is no operational reason in this phase for a swappable limiter (no testing seam need, no feature-flag override planned). The boot-time fail-closed invariant is therefore **not applicable** — nothing to verify because nothing can fail.

**Trade-off accepted:**
- (+) Zero boot-panic risk for the limiter wiring path; one fewer thing for an operator to misconfigure.
- (−) No way to inject a custom limiter (e.g., a NoopLimiter for an integration test, or a Redis-backed limiter for multi-node). When that need materializes, retrofit DI at that point — until then YAGNI.
- (−) The marker-token cleanup landed: the dead `PreviewInviteRateLimiter` / `SignupRateLimiter` newtype wrappers in `rim_infra_services/src/registration_rate_limiter.rs` were removed (per review finding SF1), and the cosmetic `let _ = &..._rate_limiter;` lines in `rim_server/src/app.rs` were replaced with real wiring: the `PreviewInviteRateLimiterToken` / `SignupRateLimiterToken` marker types in `rim_services::auth::dto` are now passed through `RegistrationContext` (`app.rs:264-297`) and annotated with `// marker: see spec §4.7 R2`. Only the marker-token sentinel pattern remains, kept as a typed seam for a future swap to a Redis-backed limiter.

### 4.8 Login path unaffected

The `/login` route and the `login` mutation are unchanged. They authenticate existing users only. They never create accounts.

---

## 5. Roles After Signup — bootstrap user lands in empty state

**Confirmed:** bootstrap user → no workspace, no auto-mint. They land in a "no workspaces yet, create one" UI state. They use the existing `createServer` mutation to spin up their first workspace, becoming `ServerMemberRole::Owner` on it via the existing path in `bootstrap` (the `bootstrap` GraphQL mutation, which is a separate concept tied to `BOOTSTRAP_SECRET` env var, will be **deprecated** by this change — see §10).

**No "instance admin" concept.** Instance admin === "anyone with shell access to issue another bootstrap link or instance invite." This is fine for now; we are explicitly not introducing an `is_instance_admin` flag, and we should not. When that need actually materializes (multi-operator instances), it will be its own design under "Instance Roles RFC". Pushing back is unnecessary; instinct is right.

**Instance invites in this phase** are issued via a new admin GraphQL mutation `createInstanceInvite` that requires `Actor` to be a `ServerMember` with role `Owner` on **at least one** server (the cheapest available proxy for "trusted operator user"). This is admittedly a hack — see §11.1 open question.

---

## 6. Three Security Flags — Resolutions

### 6.1 Bootstrap CLI link single-use + short-TTL

- **TTL:** 15 minutes hardcoded baseline; configurable via `RIM_BOOTSTRAP_TOKEN_TTL_SECONDS` with a hard cap of 3600 enforced at `RIMConfig::load`.
- **Single-use:** `bootstrap_token.used_at` set in the same transaction as `User` creation. Attempt-to-reuse fails with `INVALID_TOKEN`. Tested by `bootstrap_token_single_use_enforced` (see §8).
- **Revocation:** `revoked_at` column. New CLI subcommand `bootstrap-user revoke --token-id <uuid>` zeros it. Not in v1 scope of the spec but the column ships now.

### 6.2 Invite-link sprawl — TTL + revocation

- **`instance_invite.expires_at`** is `NOT NULL` at the DB layer (unlike workspace invites where it's nullable). At the *service layer*, `createInstanceInvite` with `expires_at = None` defaults to **`now() + 7d`** (R1: canonical). Operator-supplied values past **30 days** from now return `Validation`. No forever-tokens. The `NOT NULL` constraint is a defense-in-depth backstop — under normal flow the service always supplies a value (default or operator-provided), so the constraint should never fire.
- **Revocation:** `revoked_at` column + `revokeInstanceInvite` mutation, mirroring existing `revoke_invite`.

  > **Correction 2026-10-19:** no `revoke_invite` existed for workspace invites when this was written, nor any way for an owner to revoke one. Owners revoke them with `revokeInvite`, and they now expire like instance invites (7 days by default, 30 at most). `docs/messaging.md` "Workspace invites" has the contract.
- **Single-use vs multi-use:** instance invites carry `max_uses`, defaulting to **1**, with a hard cap of **50**. Each consumption increments `use_count`. `try_consume` mirrors the existing workspace-invite implementation.
- **Audit:** every issuance and consumption logs at `target = "audit"` with `instance_invite_id` and (on consumption) `consumed_by_user_id`.

### 6.3 `RIM_OPEN_REGISTRATION=true` — startup warning + fail-closed precondition

At server boot, in `rim_server::app::build_app_runtime`, after config load and before schema construction:

1. If `mode == Open`:
   - Emit `tracing::warn!(target = "audit", "RIM_OPEN_REGISTRATION=true: open registration mode is active; instance accepts signups without invitation");` (single log line, structured).
   - ~~Verify the rate limiter for the `signup` key is wired into the runtime. If not, fail-closed.~~ **RESOLVED (R2): not applicable.** The signup rate limiter is built inside `DefaultAuthService::new()` and cannot be absent. The check has nothing to verify.
2. If `mode == InviteOnly`: no warning. (Rate limiter still exists, but the signup mutation never reaches the rate-limit branch.)

~~The fail-closed check is implemented as a startup invariant ... See test `boot_fails_when_open_mode_without_rate_limiter`.~~ **RESOLVED (R2):** the warning log line stays (it's the operator's nudge); the fail-closed check is dropped because there is no failure mode to catch. The marker-token cleanup also landed: the dead `PreviewInviteRateLimiter` / `SignupRateLimiter` newtype wrappers were removed from `rim_infra_services/src/registration_rate_limiter.rs`, and `app.rs:264-282` no longer holds `let _ = &..._rate_limiter;` placeholders — the marker tokens are now wired through `RegistrationContext` with `// marker: see spec §6.3 R2` annotations.

---

## 7. Schema & Migrations

### 7.1 Migration name

Use `sea migrate generate add_registration_invites` (per `CLAUDE.md`, ALWAYS use the generator, never hand-name files). The resulting file will be `m{timestamp}_add_registration_invites.rs` — for this spec we refer to it as **`m20260428_add_registration_invites`**.

Single migration covers both new tables — they're a single feature unit and both ship together; one migration keeps `migrate up`/`migrate down` atomic.

### 7.2 Tables

#### `bootstrap_token`

| Column | Type | Constraint |
|---|---|---|
| `id` | uuid | PK |
| `email` | string(254) | NOT NULL — informational only, used for stdout display + audit |
| `token_hash` | string(128) | NOT NULL, UNIQUE |
| `created_at` | timestamptz | NOT NULL, default `now()` |
| `expires_at` | timestamptz | NOT NULL |
| `used_at` | timestamptz | NULL |
| `consumed_by_user_id` | uuid | NULL, FK → `user.id` `ON DELETE SET NULL` |
| `revoked_at` | timestamptz | NULL |

Index: `idx-bootstrap-token-hash-uniq` on `token_hash` (unique).

#### `instance_invite`

| Column | Type | Constraint |
|---|---|---|
| `id` | uuid | PK |
| `created_by_user_id` | uuid | NOT NULL, FK → `user.id` `ON DELETE Restrict` |
| `token_hash` | string(128) | NOT NULL, UNIQUE |
| `created_at` | timestamptz | NOT NULL, default `now()` |
| `expires_at` | timestamptz | **NOT NULL** (mandatory TTL) |
| `max_uses` | integer | NOT NULL, default 1, CHECK `max_uses > 0 AND max_uses <= 50` |
| `use_count` | integer | NOT NULL, default 0, CHECK `use_count >= 0` |
| `revoked_at` | timestamptz | NULL |

Index: `idx-instance-invite-hash-uniq` on `token_hash` (unique).
Index: `idx-instance-invite-active` on `(revoked_at, expires_at)` — supports admin "list active instance invites" without scanning consumed/expired rows.

### 7.3 Up SQL sketch

```sql
CREATE TABLE bootstrap_token (
    id uuid PRIMARY KEY,
    email varchar(254) NOT NULL,
    token_hash varchar(128) NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    used_at timestamptz,
    consumed_by_user_id uuid REFERENCES "user"(id) ON DELETE SET NULL,
    revoked_at timestamptz
);
CREATE UNIQUE INDEX "idx-bootstrap-token-hash-uniq" ON bootstrap_token(token_hash);

CREATE TABLE instance_invite (
    id uuid PRIMARY KEY,
    created_by_user_id uuid NOT NULL REFERENCES "user"(id) ON DELETE RESTRICT,
    token_hash varchar(128) NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    max_uses integer NOT NULL DEFAULT 1 CHECK (max_uses > 0 AND max_uses <= 50),
    use_count integer NOT NULL DEFAULT 0 CHECK (use_count >= 0),
    revoked_at timestamptz
);
CREATE UNIQUE INDEX "idx-instance-invite-hash-uniq" ON instance_invite(token_hash);
CREATE INDEX "idx-instance-invite-active" ON instance_invite(revoked_at, expires_at);
```

### 7.4 Down SQL

```sql
DROP TABLE instance_invite;
DROP TABLE bootstrap_token;
```

### 7.5 Rollback note

Down is a `DROP TABLE` and is **destructive of all bootstrap and instance-invite tokens**. Rollback is acceptable in dev; in prod, rollback is only acceptable when the deployment also rolls back to a binary that does not ship `signupWithInvite` — the FE will still expect the mutation otherwise. Existing `user`, `session`, `invite` data is untouched by `down`. Do **not** run `down` against a prod DB without operator sign-off; the tracking issue must explicitly call out the data loss.

### 7.6 Migration file registration

`migration/src/lib.rs` `migrations()` vec gets one new entry appended:

```rust
Box::new(m20260428_add_registration_invites::Migration),
```

And one new `mod m20260428_add_registration_invites;` line at the bottom.

---

## 8. Test List (Required)

Every new function gets unit tests AND rustdoc. Implementer: if either is missing, this comes back. No exceptions.

### 8.1 Unit tests (`rim_infra_services::auth_unit_tests` — using `mockall`)

1. `bootstrap_token_single_use_enforced` — issuing a token, consuming it, then attempting a second consumption returns `InvalidToken`.
2. `bootstrap_token_expired_rejected` — token with `expires_at < now()` returns `ExpiredToken`.
3. `bootstrap_token_revoked_rejected` — token with `revoked_at IS NOT NULL` returns `InvalidToken` (NOT `ExpiredToken` — revocation must be indistinguishable from invalid).
4. `bootstrap_token_consumed_rejected_indistinguishable_from_invalid` — token with `used_at IS NOT NULL` returns `InvalidToken` (same code as missing token; verifies the FE-leak rule).
5. `bootstrap_token_creates_user_with_no_workspace` — successful consumption produces a user with no `server_member` row.
6. `instance_invite_single_use_enforced` — `max_uses=1`, second consumption fails with `InvalidToken`.
7. `instance_invite_multi_use_allowed_until_exhausted` — `max_uses=3`, three consumptions succeed, fourth fails with `InvalidToken`.
8. `instance_invite_expired_rejected` — `EXPIRED_TOKEN`.
9. `instance_invite_revoked_rejected` — `INVALID_TOKEN`.
10. ~~`instance_invite_required_expires_at_validation`~~ **DROPPED (R1).** Contradicted Test 12. Spec §6.2 says "Default TTL when not supplied to `createInstanceInvite`: **7 days**" — that establishes a default, not a required-field contract. The DB column remains `NOT NULL` (defense in depth), but the *service-layer* contract is "null in → 7d default out", not "null in → Validation error". Test 12 is the canonical behavior.
11. `instance_invite_max_uses_cap_enforced` — `createInstanceInvite` with `max_uses > 50` returns `Validation`.
12. `instance_invite_default_ttl_seven_days` — `createInstanceInvite` with no `expires_at` defaults to `now() + 7d`. **(R1: this is the canonical behavior; Test 10 dropped.)** Add a sub-assertion: `expires_at > now() + 6d AND expires_at < now() + 8d` (loose bounds tolerate clock skew in CI).
13. `signup_invite_only_rejects_no_token` — `RegistrationMode::InviteOnly` and zero tokens → `Validation`.
14. `signup_invite_only_rejects_multiple_tokens` — invite-only and ≥2 tokens → `Validation`.
15. `signup_open_rejects_token_present` — `RegistrationMode::Open` with any token → `Validation`.
16. `signup_open_accepts_no_token` — open mode, no tokens, valid creds → `SignupResultDto`.
17. `signup_open_rate_limited_after_threshold` — N+1th call within window returns `RateLimited`.
18. `signup_username_already_taken_returns_conflict` — `Conflict` even with valid token.
19. `signup_password_too_short_returns_validation` — same `validate_password` rules as existing service.
20. `preview_invite_token_workspace_returns_workspace_payload` — valid workspace invite token returns `tokenType: WORKSPACE`, populated `workspace` field with server name + inviter display name; verifies no `used_at`/`use_count` mutation on the underlying row (re-read after preview, assert unchanged).
21. `preview_invite_token_instance_returns_minimal_payload` — valid instance invite returns `tokenType: INSTANCE`, `expiresAt` set, `workspace` is null; verifies no mutation.
22. `preview_invite_token_bootstrap_returns_bootstrap_type` — valid bootstrap token surfaces as `tokenType: BOOTSTRAP` (distinct from `INSTANCE`; FE uses the discriminator to pick which `signupWithInvite` input field carries the raw token, even though it shares "Create your RIM account" copy with `INSTANCE`).
23. `preview_invite_token_expired_returns_expired_token` — token past `expires_at` returns `ExpiredToken` (distinguishable code, matches §4.5 rule).
24. `preview_invite_token_invalid_returns_invalid_token` — missing / consumed / revoked all collapse to `InvalidToken` (indistinguishable, three sub-cases in one parametrized test).
25. `preview_invite_token_rate_limited_after_threshold` — 31st call from same IP within 5min window returns `RateLimited`; 30 succeed.

### 8.2 Integration tests (`rim_infra_db/tests` and `rim_api/tests`)

26. `bootstrap_token_repo_round_trip` (db) — `BootstrapTokenRepository` create / get_by_hash / mark_used / revoke.
27. `instance_invite_repo_round_trip` (db) — `InstanceInviteRepository` create / get_by_hash / try_consume / revoke / list_active.
28. `signup_with_invite_e2e_invite_only` (graphql e2e) — boot in invite-only mode, run `signupWithInvite` with a freshly-created `bootstrap_token`, verify session cookie + CSRF set.
29. `signup_with_invite_e2e_open_mode` (graphql e2e) — boot in open mode, signup without token succeeds.

**Resolver-level tests (R3) — three additional integration tests, placement decided:**

Placed in **`rim_api/tests/registration_resolvers_e2e.rs`** behind the **`e2e-tests`** feature, requiring `DATABASE_URL`. Rationale: these three tests exercise the *full* GraphQL resolver path (parse input → resolve via context → call into a real `DefaultAuthService` over a real DB) and are the canonical proof-of-wiring for what the frontend will hit. Schema-only unit variants are already covered for the *behavior* in §8.1 Tests 20–25 via `rim_api/src/tests.rs` (mock `AuthService`, schema-only). The integration tests below are *additional* — they verify the wiring, not just the resolver code.

13. `query_registration_mode_returns_invite_only_by_default` — boot runtime in default config (no `RIM_OPEN_REGISTRATION`), execute `query { registrationMode }`, assert `INVITE_ONLY`. Reuse the `e2e-tests` boot fixture in `rim_api/tests/e2e_tests_guard.rs` for runtime construction.
14. `query_preview_invite_token_workspace_happy_path` — seed: create a user via repo helpers, create a workspace, create an `invite` row. Then `query { previewInviteToken(token: "...") { tokenType expiresAt workspace { serverName invitedByDisplayName } } }`. Assert `WORKSPACE`, populated workspace fields, and (re-fetch the underlying `invite` row) `use_count` unchanged.
15. `mutation_signup_with_invite_bootstrap_happy_path` — seed: create a `bootstrap_token` row via `BootstrapTokenRepository`. Run `mutation { signupWithInvite(input: { username, password, bootstrapToken }) { userId serverId } }`. Assert `userId` non-null, `serverId` is null. Re-fetch token: `used_at IS NOT NULL`, `consumed_by_user_id == userId`. Assert no `server_member` row for that user.

**Helper-fixture requirements:**
- A new `rim_api/tests/support/registration.rs` helper module with:
  - `async fn seed_bootstrap_token(db: &DatabaseConnection, raw_token: &str, expires_at: DateTime<Utc>) -> BootstrapTokenId`
  - `async fn seed_instance_invite(db: &DatabaseConnection, created_by: UserId, raw_token: &str, opts: SeedInstanceInviteOpts) -> InstanceInviteId`
  - `async fn seed_workspace_invite(db: &DatabaseConnection, owner: UserId, server: ServerId, raw_token: &str) -> InviteId`
- These wrap the `*Repository::create_*` methods plus a `rim_auth::session_token::hash_token` call so test code passes the *raw* token in and the helper handles the hash.
- Reuse the existing `boot_test_runtime()` / `test_db_pool()` fixtures from `rim_api/tests/e2e_tests_guard.rs`. Do not duplicate config bootstrap.

### 8.3 Boot-time tests (`rim_server` integration)

30. `boot_emits_warning_on_open_mode` — start the runtime with `RIM_OPEN_REGISTRATION=true`; assert a `tracing::warn!` line containing `"RIM_OPEN_REGISTRATION=true"` is emitted (use a `tracing-test` capture or equivalent). **(Kept — this verifies the operator-visible nudge, which is still wired.)**
31. ~~`boot_fails_when_open_mode_without_rate_limiter`~~ **DROPPED (R2).** Rate limiter is constructed inside `DefaultAuthService::new()`; "missing" is unrepresentable.
32. ~~`boot_fails_when_preview_rate_limiter_missing`~~ **DROPPED (R2).** Same reason.

**Total: 30 tests** (was 32; R2 drops 2; R1 drops 1; R3 adds 3 — net +0 against the original 32, but composition changed: 25 unit + 2 db + 2 e2e + 3 resolver-e2e + 1 boot-warning = 33. Updated floor: **33 tests**.) The implementer may add more.

> **See Amendment 2026-04-28b (§13)** — adds 7 tests on top, raising the floor to **40 tests**.

### 8.4 Rustdoc requirements

Every new public function gets `///` doc with:
- One-line summary.
- `# Errors` section listing every `ServiceError` variant it can return and under what condition.
- `# Examples` section for service-trait methods (mock-based).

Specifically: `AuthService::signup_with_bootstrap`, `AuthService::signup_with_instance_invite`, `AuthService::signup_open`, `AuthService::create_instance_invite`, `AuthService::revoke_instance_invite`, `BootstrapTokenRepository::*`, `InstanceInviteRepository::*`. No exceptions; ship without docs and the review will block.

---

## 9. GraphQL SDL Diff

This is what the frontend will see after re-running `npm run codegen`. Listed for the parallel frontend spec.

### 9.1 New input types

> **See Amendment 2026-04-28b (§13)** — `SignupWithInviteInput` adds `email: String!` and `displayName: String`. SDL diff for the amended shape is in §13.2.

```graphql
input SignupWithInviteInput {
  username: String!
  password: String!
  bootstrapToken: String
  instanceInviteToken: String
  workspaceInviteToken: String
}

input CreateInstanceInviteInput {
  expiresAt: DateTime          # optional; defaults to now() + 7 days
  maxUses: Int                 # optional; defaults to 1
}
```

### 9.2 New payload types

```graphql
type SignupWithInvitePayload {
  userId: ID!
  serverId: ID                 # nullable; only set when workspaceInviteToken was used
}

type InstanceInvite {
  id: ID!
  createdByUserId: ID!
  expiresAt: DateTime!
  maxUses: Int!
  useCount: Int!
  revokedAt: DateTime
  createdAt: DateTime!
}

type CreateInstanceInvitePayload {
  inviteToken: String!         # the raw token, shown to admin once
  invite: InstanceInvite!
}
```

### 9.3 New mutations

```graphql
extend type Mutation {
  signupWithInvite(input: SignupWithInviteInput!): SignupWithInvitePayload!
  createInstanceInvite(input: CreateInstanceInviteInput!): CreateInstanceInvitePayload!
  revokeInstanceInvite(inviteId: ID!): Boolean!
}
```

### 9.4 New queries

```graphql
extend type Query {
  registrationMode: RegistrationMode!                      # so /login can decide whether to render "Sign up" link
  listInstanceInvites: [InstanceInvite!]!                  # admin-only; PermissionDenied otherwise
  previewInviteToken(token: String!): InviteTokenPreview!  # unauthenticated; rate-limited 30/5min/IP
}

enum RegistrationMode {
  INVITE_ONLY
  OPEN
}

enum InviteTokenType {
  BOOTSTRAP
  INSTANCE
  WORKSPACE
}

"""
Read-only preview of a registration token. Does NOT consume the token.
Returned fields are minimized per token type to avoid leaking inviter
identity on instance invites. See spec §4.7.
"""
type InviteTokenPreview {
  tokenType: InviteTokenType!
  expiresAt: DateTime          # nullable: workspace invites historically allow null expires_at
  workspace: WorkspaceInvitePreview   # populated only when tokenType == WORKSPACE
}

type WorkspaceInvitePreview {
  serverId: ID!
  serverName: String!
  invitedByDisplayName: String!
}
```

### 9.5 New error codes

| `extensions.code` | When |
|---|---|
| `INVALID_TOKEN` | Token missing/consumed/revoked. |
| `EXPIRED_TOKEN` | Token exists but past `expires_at`. |
| `EMAIL_TAKEN` | Signup email collides with an existing user (case-insensitive). See Amendment 2026-04-28b §13. |

`map_service_error` in `rim_api/src/errors.rs` gains two new arms for `ServiceError::InvalidToken` and `ServiceError::ExpiredToken`. Amendment 2026-04-28b adds a third arm for `ServiceError::EmailTaken`.

### 9.6 Deprecated

`bootstrap` mutation (the existing one tied to `RIM_BOOTSTRAP_SECRET`) is marked `@deprecated(reason: "Use signupWithInvite with a bootstrapToken issued by the rim_server bootstrap-user CLI command")`. Not removed in this phase. The `BOOTSTRAP_SECRET` env var path stays in for one minor release for rollback.

---

## 10. Rollout

1. Migration applied. Both new tables created. Existing `invite` untouched.
2. Server ships with `RegistrationMode::InviteOnly` default. No env flag flip needed.
3. Operator runs `cargo run -p rim_server -- bootstrap-user --email me@example.com`, copies the link, signs up at `/join/<raw_token>` (path-param form). They land in empty state.
4. Operator creates a workspace. Now they're a workspace owner. They can issue workspace invites (existing flow) AND instance invites (new flow).
5. Existing `bootstrap` GraphQL mutation continues to work (deprecated, scheduled for removal one minor release later). `RIM_BOOTSTRAP_SECRET` continues to be honored.
6. To enable open mode on a deployment: set `RIM_OPEN_REGISTRATION=true`, ensure rate limiter is built into the runtime (it always is — see §6.3 — but if the wiring is ever removed accidentally, the boot check catches it).

---

## 11. Out of Scope (Explicit)

- Email-domain allowlist for open mode. Phase-7 follow-up; spec hook is `RIM_OPEN_REGISTRATION_ALLOWED_DOMAINS=foo.com,bar.com` (not implemented).
- SAML / OIDC JIT provisioning.
- Frontend implementation (the frontend runs in parallel).
- Plugin marketplace registration.
- Multi-node coordination of the signup rate limiter (single-node `DashMap`-based rate limiter is fine for now per `rim_infra_services::command::rate_limiter`).
- Email delivery of bootstrap/instance invite links. Operator copies the stdout URL by hand or via shell pipe.

### 11.1 Open question — flag back to the coordinator / Jakub

**The "anyone with at least one Owner ServerMember" gate for `createInstanceInvite` is a placeholder.** It says "if you own a workspace on this instance, you can mint instance-level invites that create accounts not bound to your workspace." That bleeds workspace-owner authority into instance-admin authority. It's defensible for now (closed beta, single-operator instances), but if RIM ever ships to a deployment with multiple unrelated workspace owners on one instance, this is the wrong model.

**Decision needed from Jakub:** is this acceptable for the current phase, or do we ship `createInstanceInvite` gated only on the operator's CLI (i.e., **no GraphQL mutation at all in this phase, all instance invites issued from `rim_server` CLI**)? The CLI-only path is more conservative, smaller surface area, and dodges the question entirely. I lean toward CLI-only. Coordinator — confirm.

---

## 12. Handoff Checklist for the Implementer

Before opening review:

- [ ] Migration generated with `sea migrate generate add_registration_invites`, both tables created, registered in `migration/src/lib.rs`.
- [ ] `entity/` updated via `sea generate entity -l -o /tmp/rim-entities` and merged manually (per CLAUDE.md — never regenerate directly into `entity/src/`).
- [ ] `BootstrapTokenRepository` and `InstanceInviteRepository` traits in `rim_core/src/auth/repository.rs`, impls in `rim_infra_db/src/sea_orm_impl/`.
- [ ] `AuthService` extended with three new methods + two new admin methods (instance-invite create/revoke); each is documented and tested.
- [ ] `RegistrationMode` enum lives in `rim_services::auth::dto` (or a new `registration` module), parsed in `rim_server::config`, injected into GraphQL context.
- [ ] `signupWithInvite`, `createInstanceInvite`, `revokeInstanceInvite`, `listInstanceInvites`, `registrationMode`, `previewInviteToken` GraphQL items added in `rim_api/src/{mutation,query,types}.rs`.
- [ ] `PreviewInviteRateLimiter` (30 req / 5min / IP) constructed inside `DefaultAuthService::new()`. **(R2: no DI; no boot-fail invariant.)** ~~Cosmetic `let _ = &..._rate_limiter;` lines in `app.rs:264-282` cleaned up — either remove the marker tokens entirely or annotate them with `// marker: see spec §4.7 R2`.~~ **DONE (R2):** the cosmetic `let _ = &..._rate_limiter;` placeholders were replaced with real wiring through `RegistrationContext` (`app.rs:264-297`) and annotated with `// marker: see spec §4.7 R2`.
- [ ] Two new `ServiceError` variants (`InvalidToken`, `ExpiredToken`) + matching arms in `map_service_error`.
- [ ] Boot-time validation in `build_app_runtime` for `RIM_OPEN_REGISTRATION=true` — `tracing::warn!` only. **(R2: fail-closed check dropped.)**
- [ ] All 40 tests written and passing under `cargo test --workspace -- --test-threads=1` (25 unit + 2 db-integration + 2 e2e + 3 resolver-e2e + 1 boot-warning + 7 amendment tests per §13.4). Resolver-e2e tests live in `rim_api/tests/registration_resolvers_e2e.rs` under the `e2e-tests` feature.
- [ ] `cargo fmt --all` + `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `/api/sdl` re-fetched into `rim-ui/schema.graphql` and frontend codegen run.
- [ ] Review opened. BLOCK findings fixed before reporting done.
- [ ] No `git commit` / `git push` / `git merge` from your end. Surface a PR; the coordinator merges.

---

## Amendment 2026-04-28b — email + displayName at signup

**Status:** Spec amendment. Backend and frontend for the original spec have already landed; this amendment closes a real product gap surfaced post-impl by the frontend implementer (no email = no password reset, no notifications, no recovery). Jakub's call: full redesign — email is required at signup, not deferred to a later "claim email" flow.

**Author:** the backend architect.
**Date:** 2026-04-28 (b suffix to disambiguate from the original same-day rev).
**Scope:** input shape, validation, schema migration for `user.email`, error mapping, tests. No changes to bootstrap/instance-invite token flow itself — they just plumb two more fields through the cmd → service → repo path.

### 13.1 Rationale

`SignupWithInviteInput` shipped as `username + password` plus token fields. That was wrong for a multi-tenant communication product:

- **No password reset path.** `password_reset_token` already exists in schema; without an email column on `user`, there is no way to send the reset link to anyone. Reset is structurally impossible.
- **No notifications.** Mentions, DMs, workspace invites — none can route off-platform.
- **No account recovery.** Lost-username case has no out-of-band identifier to verify.

Partial fix (a) — accept email as optional, prompt later — was rejected because (a) downstream features (notifications, reset) end up branching on `email.is_some()` indefinitely, (b) operators can't enforce "every account has a deliverable email" if some accounts predate the requirement, and (c) the migration to "email required" later is strictly harder than getting it right now while we're already in the registration code.

**Decision: full redesign — `email` required, `displayName` optional with username fallback.**

### 13.2 SDL diff

**Before** (current shipped):

```graphql
input SignupWithInviteInput {
  username: String!
  password: String!
  bootstrapToken: String
  instanceInviteToken: String
  workspaceInviteToken: String
}
```

**After**:

```graphql
input SignupWithInviteInput {
  username: String!
  email: String!                # NEW — required
  displayName: String           # NEW — optional; falls back to username server-side
  password: String!
  bootstrapToken: String
  instanceInviteToken: String
  workspaceInviteToken: String
}
```

`SignupWithInvitePayload` is unchanged. No new GraphQL types are introduced; only the input gains two fields and the error-code surface gains `EMAIL_TAKEN` (see §13.5).

### 13.3 Validation rules

#### `email: String!` (required)

| Rule | Decision |
|---|---|
| Format | Pragmatic regex, **not** a full RFC 5322 implementation. Pattern: `^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$`. Rationale: RFC 5322 accepts addresses no real mail provider will deliver (quoted strings, comments, IP literals); we are validating "is this plausibly a deliverable email," not "is this a syntactically conformant 5322 address." If we ever need 5322 strictness, swap the regex for the `validator` crate's `validate_email` later — same call site. |
| Length | `1..=254` chars, where 254 is the RFC 5321 hard cap on the address part. Reject empty after trim. |
| Case | **Lowercased on insert.** All comparisons (uniqueness check, future password-reset lookup) use the lowercased form. The DB column is `varchar(254) NOT NULL` and a unique index is built on `LOWER(email)` so the constraint is case-insensitive at the schema layer too — defense in depth against a non-normalized write path. |
| Uniqueness | Must be unique across all users in the instance. Collision returns `ServiceError::EmailTaken` → GraphQL `EMAIL_TAKEN`. Match the existing username uniqueness convention. |
| Trim | Outer whitespace trimmed before validation and storage. Internal whitespace inside the local part is not allowed by the regex; no further normalization. |

The validator implementation lives in `rim_infra_services/src/auth.rs` next to `validate_username` / `validate_password`, as `fn validate_email(email: &str) -> ServiceResult<String>` — note it returns the **normalized (trimmed + lowercased) form**, not `()`. Service code uses the returned `String` for both the uniqueness check and the insert. This pattern matches "validate-and-canonicalize at the boundary"; the rest of the system never sees the raw input.

#### `displayName: String` (optional)

| Rule | Decision |
|---|---|
| Optional | Yes. If absent or empty after trim, fall back to `username` at user-creation time. The `User` entity field stays `Option<String>` (already shipped), and the fallback is "if input was None or empty, store `username` as `display_name`". |
| Length | `1..=80` chars **when present** (post-trim). 80 is wider than `username`'s 32 because display names allow spaces and Unicode; 80 is conventional (Slack uses 80, Discord uses 32, Matrix is unbounded — 80 is the middle path that fits a sidebar without truncation). |
| Trim | Leading/trailing whitespace trimmed. If the result is empty, treat as "absent" and apply the username fallback. Do **not** reject — the user submitted "blank means use my username," and the cleaner UX is silent fallback rather than a validation error. |
| Charset | No restriction beyond "non-empty after trim, ≤80 chars." Unicode allowed. Newlines/tabs are stripped by the trim step. |

Implemented as `fn normalize_display_name(input: Option<&str>, username: &str) -> ServiceResult<String>` in `rim_infra_services/src/auth.rs`. Returns the final string to insert. Length-violation returns `ServiceError::Validation`.

### 13.4 Migration plan

`user.display_name` already exists on the entity (`entity/src/user.rs:15` — `pub display_name: Option<String>`) and in the database. **No migration needed for displayName.** The amendment merely starts populating it from input.

`user.email` does **not** exist. **Migration required.**

**Generation:** `sea migrate generate add_user_email` (per CLAUDE.md — never hand-name).
**Refer to it as** `m20260428_add_user_email`.

#### Up SQL

```sql
-- Phase 1: add column NULLable so existing rows are not rejected.
ALTER TABLE "user" ADD COLUMN email varchar(254);

-- Phase 2: backfill. Generate a synthetic placeholder for every existing row.
-- Rationale: the only existing user(s) at the time of this migration are the
-- bootstrap operator(s) created via the original (pre-amendment) signup flow;
-- they need a valid string in the column to satisfy NOT NULL on phase 3.
-- Synthetic value is "<username>@local.invalid" lowercased — RFC 2606 reserves
-- `.invalid` for exactly this kind of placeholder, and operators MUST update
-- their own row via a future "edit profile" mutation before relying on
-- password reset or notifications. The audit log line on migration apply
-- (logged from migration `up()`) makes this requirement explicit.
UPDATE "user"
   SET email = LOWER(username) || '@local.invalid'
 WHERE email IS NULL;

-- Phase 3: enforce NOT NULL now that every row has a value.
ALTER TABLE "user" ALTER COLUMN email SET NOT NULL;

-- Phase 4: case-insensitive unique index.
CREATE UNIQUE INDEX "idx_user_email_lower_unique"
    ON "user" (LOWER(email));
```

All four phases run in a single migration `up()` so they're atomic at the SeaORM level.

#### Down SQL

```sql
DROP INDEX IF EXISTS "idx_user_email_lower_unique";
ALTER TABLE "user" DROP COLUMN email;
```

#### Rollback note

Down drops the `email` column and is **destructive of all per-user email data**, including any real emails captured between amendment ship and rollback. Rollback is acceptable only if the deployment also rolls back to a binary where `signupWithInvite` does not require `email` in its input — otherwise new signups will fail at the resolver. In dev, drop-and-rebuild is fine. In prod, rollback requires operator sign-off; the tracking issue must explicitly call out the data loss.

#### Entity update

Re-generate entities to a temp dir per CLAUDE.md, then merge `email: String` into `entity/src/user.rs` Model manually. The entity field is **non-optional** `String` (NOT NULL at DB layer). Any existing mapper code that constructs a `Model` literal will fail to compile until `email` is set — this is intended; it surfaces every construction site that needs to be amended.

### 13.5 Error mapping

New `ServiceError` variant:

```rust
#[error("email already in use")]
EmailTaken,
```

`map_service_error` in `rim_api/src/errors.rs` gains a new arm:

```rust
ServiceError::EmailTaken => extension_error("email already in use", "EMAIL_TAKEN"),
```

The `EMAIL_TAKEN` code is **distinguishable from** generic `CONFLICT` (which already exists for username collisions). Two reasons to keep them separate:
1. The frontend wants to highlight the email field specifically when it collides — that requires a discriminator.
2. Reusing `CONFLICT` means string-sniffing the message to know which field was the problem, which violates the no-stringly-typed-errors rule.

The "indistinguishability of consumed/invalid tokens" principle from §4.6 does **not** apply to email collisions — there is no security harm in confirming "this email is in use" because that information is already obtainable through the `forgotPassword` flow's per-email rate limit, and obscuring it just degrades the signup UX.

### 13.6 Test additions

Seven new tests, on top of the existing 33. New floor: **40 tests**.

Unit tests in `rim_infra_services::auth_unit_tests`:

33. `signup_with_email_happy_path` — valid email + token → `SignupResultDto`; verify the persisted `User.email` is lowercased and trimmed (e.g., input `"  Foo@Bar.COM "` persists as `"foo@bar.com"`).
34. `signup_duplicate_email_returns_email_taken` — pre-seed a user with `email = "x@y.z"`, attempt signup with `"X@Y.Z"` (different case) → `ServiceError::EmailTaken`. Verifies case-insensitive uniqueness.
35. `signup_malformed_email_returns_validation` — parametrized over: `"plain"`, `"@host.com"`, `"a@"`, `"a@b"`, `"a b@c.com"`, empty string, 255-char string. Each returns `ServiceError::Validation`.
36. `signup_displayname_omitted_falls_back_to_username` — input `displayName: None`, username `"alice"`; persisted user has `display_name == Some("alice")`.
37. `signup_displayname_blank_falls_back_to_username` — input `displayName: Some("   ")`, username `"alice"`; persisted `display_name == Some("alice")`. Verifies trim-then-fallback, **not** validation error.
38. `signup_displayname_too_long_returns_validation` — input is 81 chars after trim → `ServiceError::Validation`.

Resolver-e2e in `rim_api/tests/registration_resolvers_e2e.rs` under `e2e-tests`:

39. `mutation_signup_with_invite_missing_email_rejected_by_schema` — execute `mutation { signupWithInvite(input: { username, password, bootstrapToken }) ... }` with no `email` field. Assert the GraphQL response is a *parse/validation* error (the schema requires `email: String!`), **not** a resolver error. This proves the SDL contract.

(Note: the displayName-fallback case is already covered by Test 36/37 at the unit layer; we don't duplicate it at the e2e layer.)

Every new function gets rustdoc per §8.4 — `validate_email`, `normalize_display_name`, the new `EmailTaken` arm in `map_service_error`, and any new repo method (e.g., `UserRepository::find_by_email_lower` if the implementer needs one for the uniqueness check) all ship with `# Errors` and `# Examples` sections. No exceptions.

### 13.7 Cmd / DTO changes

The `SignupWithBootstrapCmd`, `SignupWithInstanceInviteCmd`, and `SignupOpenCmd` (the three new commands fed by the `signupWithInvite` resolver) all gain two fields:

```rust
pub email: String,                  // required, pre-normalized at the resolver boundary
pub display_name: Option<String>,   // raw input; service decides fallback
```

The resolver normalizes `email` (trim + lowercase) before constructing the cmd, but the service **also** runs `validate_email` on the way in — defense in depth, in case a future caller bypasses the resolver. `display_name` is passed through raw; `normalize_display_name` is called inside the service, since the username fallback needs the username from the same cmd.

`AcceptInviteCmd` is intentionally **excluded** per Q2 ruling — see the supersedure note immediately below.

~~`AcceptInviteCmd` is the existing path (workspace-invite signup). It also gets `email` + `display_name` so workspace-invite signups are no different from instance/bootstrap signups w.r.t. the new fields. **This is a behavior change to `acceptInvite`** — that mutation's input gains the same two fields. The `AcceptInviteInput` GraphQL type adds them per the same shape; the SDL diff for `acceptInvite` is parallel to the `signupWithInvite` diff above. Tests 33–38 above all run against both paths or get a parallel pair (the implementer's call on parametrization vs. duplication).~~

**SUPERSEDED by Q2 ruling (review finding SD1).** `AcceptInviteCmd` is **not** modified. The `accept_invite` GraphQL mutation and its cmd remain at the original `(invite_token, username, password)` shape — verified against the live `AcceptInviteCmd` definition in `rim_services/src/auth/command.rs:37–41`. Amendment 2026-04-28b is scoped to `signupWithInvite` only; the workspace-invite path is intentionally left alone.

**Implementation note (legacy path):** because `user.email` is `NOT NULL` post-amendment, the legacy `accept_invite` path supplies a synthetic placeholder email of the form `"<username>@local.invalid"` (verified at `rim_infra_services/src/auth.rs:730` — `email: format!("{}@local.invalid", username.to_lowercase())`; a parallel placeholder is also issued for the deprecated `bootstrap` mutation at `rim_infra_services/src/auth.rs:462`). RFC 2606 reserves `.invalid` for exactly this kind of placeholder. Operators created via `accept_invite` must update their email through a future profile-edit flow before relying on password reset / notifications. This is the same pattern used by the `m20260428_add_user_email` migration's backfill (§13.4 phase 2).

The downstream consequence is that `signup_with_bootstrap`, `signup_with_instance_invite`, and `signup_open` all carry real `email`/`display_name` end-to-end, while `accept_invite` does not — which matches the Q2 scope decision and is the only place in the codebase where the synthetic-email path is acceptable.

### 13.8 Frontend coordination

The frontend lead needs the input-shape pre-spec so `JoinForm` changes can be planned before the frontend implementer picks up implementation. Per the team operating model, this is a SendMessage ping. **See §13.9 — operating constraint.** The agreed contract the frontend lead needs to know:

- `email: String!` — required, max 254 chars, regex per §13.3, case-insensitive uniqueness (server lowercases).
- `displayName: String` — optional, 1–80 chars when present, trimmed, falls back to `username` if absent or blank.
- New error code: `EMAIL_TAKEN` (highlight email field specifically; do not lump into generic `CONFLICT`).
- No change to existing `userId` / `serverId` payload shape.

### 13.9 Operating constraint — frontend ping

This thread does not have a SendMessage tool available; only file/grep tools are exposed to the architect in the current session. The frontend ping must be relayed by **the coordinator** (who runs the team-coordination layer) using §13.8 as the message body, kept under 100 words.

**Suggested message body (under 100 words):**

> Frontend — registration-redesign amendment 2026-04-28b. `SignupWithInviteInput` adds `email: String!` (required, max 254, server lowercases + trims, case-insensitive unique) and `displayName: String` (optional, 1–80 chars, falls back to username if absent/blank). New error code `EMAIL_TAKEN` for collisions — highlight the email field, do not reuse generic `CONFLICT`. `acceptInvite` is **out of scope** per Q2 ruling — its input is unchanged; legacy users get a synthetic `<username>@local.invalid` placeholder. Pre-plan `JoinForm` accordingly; The frontend implementer will pick up impl after review of the backend changes. Spec at `docs/decisions/registration-redesign-2026-04-28.md` §13. — the architect
