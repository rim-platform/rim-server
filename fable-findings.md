# rim-server — Review Findings

**Reviewer:** Claude Fable 5
**Date:** 2026-07-08
**Scope:** Architecture, security, and implementation quality of the full `rim-server` Rust workspace (~80k LOC, ~800 tests).
**Method:** Layered static analysis + targeted code tracing across auth/session/CSRF, authorization, injection, file handling, GraphQL, persistence, and concurrency. `cargo clippy --workspace --all-targets -- -D warnings` passes clean. `cargo audit` could **not** run in this environment (the local RustSec DB has a CVSS-4.0 entry that `cargo-audit 0.21.2` can't parse — the same wall CI will hit); dependency versions were checked manually instead.

---

## Status (re-checked against code 2026-10-04)

| Finding | Status |
|---|---|
| S-H1 reset tokens in logs | **Fixed** (Track B3 — `LogEmailSender` logs recipient/subject only) |
| S-H2 SSRF via DNS in link previews | **Fixed** (public-only DNS resolver on the fetch client) |
| S-H3 no login rate limiting | **Fixed** (30 attempts/5 min per IP; failures limited to 10/15 min per (IP, username) and 100/hour per username across all IPs). Addresses that signed in to an account (exact, case-sensitive username) in the last 30 days skip the per-username limit (still held to the per-(IP, username) one), so guessers can't lock an owner out of their usual address; a password reset forgets the account's addresses; a sustained lockout of a *new* address needs 3+ addresses (or IPv6 /64s) guessing for up to an hour. The known addresses are in memory, so a restart forgets them. Trade-off: a distributed guesser gets up to 2,400 guesses/day on one account (was 960) |
| A-M1 `mockall` in production deps | **Fixed** (moved to `[dev-dependencies]`) |
| S-M1 reset doesn't revoke sessions | **Fixed** (one transaction: atomic token claim, new password, other reset links spent, all sessions revoked). Gaps: a WebSocket already open on a revoked session keeps streaming until it disconnects (auth is checked only at connect); a login with the old password that races the reset commit can keep its session and re-mark its address as a known login source |
| S-M2 `workspacePresence` IDOR | **Fixed** (requires workspace membership) |
| S-M3 login timing oracle | **Fixed** (dummy Argon2 verify for unknown usernames; reset requests do one lookup either way) |
| S-M4 no rate limit on `requestPasswordReset` | **Fixed** (10/hour per IP; 3/hour per username, over which requests silently send nothing) |
| S-M5 storage quota not enforced | **Fixed** (declared size reserved for the 15-minute upload ticket; uploads validated before any write and capped at the declared size) |
| S-L5 reset token check-then-mark not atomic | **Fixed** (with S-M1: conditional `UPDATE ... WHERE used_at IS NULL`) |
| A-H1 `BEFORE` pagination | **Fixed (backend), verified end to end** by `rim_api/tests/messaging_e2e.rs` (resolver → service → repo → Postgres, page size 3 / 1, both directions, `next`/`prev` round trips). The e2e test reproduced the second defect: a `BEFORE` page with a cursor returned the far end of the range (from the oldest message, `BEFORE` gave the 3 newest and skipped the rest). Fixed by fetching `BEFORE` pages nearest-first and flipping them back, in every copy (`message.rs` list + FTS search, `message_thread.rs`, `conversation.rs` ×2, `incident.rs`; search also drops its overflow row from the far end in `DefaultMessageService::search_messages`), with repo tests in `rim_infra_db/tests/`. Cursorless `BEFORE` still returns the newest page (callers rely on it). Still open: the first defect is on the client side — rim-ui `useMessages.ts` loads older history with `BEFORE` + `next`, which per `docs/pagination.md` walks toward *newer* messages and hits an empty page after the first; older history is `AFTER` + `next` (asserted in the e2e test). Follow-up: merge the hand-rolled paginators into one helper |
| A-M7 e2e coverage of messaging | **Fixed** (`rim_api/tests/messaging_e2e.rs`: send, reply/threads, edit, soft delete, `messages`/`threadMessages`/`searchMessages` pagination, non-member and unauthenticated access, through GraphQL on a real database) |
| A-L11 dead `rim_services/plugin_host` | **Fixed** for the dead-code part (module deleted 2026-10-04); the `actual_state.md` doc-drift parts were not re-checked |
| A-L4 monolithic resolver files | **Fixed** (`mutation.rs`/`query.rs`/`types.rs`/`tests.rs` split per domain under `rim_api/src/{mutation,query,types,tests}/`; `Query`/`Mutation` are `MergedObject`s whose member order keeps the SDL byte-identical). `subscription.rs` (339 lines) left as one object |
| CI `cargo audit` warning-only (strengths honesty note) | **Fixed** (`security_audit` job is blocking; ignores with justifications in `.cargo/audit.toml`) |

Everything not listed has not been re-checked. Update this table when fixing a finding.

---

## Overall assessment

This is a mature, security-conscious backend. The layered architecture (`rim_server → rim_api → rim_infra_services → rim_infra_db → entity`, with contracts in `rim_core`/`rim_services`) is **enforced in practice, not just documented**: greps confirm zero infra/SeaORM imports leaking into `rim_api`, zero transport/Actix/GraphQL concerns in `rim_core`/`rim_services`, and the only infra reach from `rim_server` is the sanctioned composition root (`app.rs`, FL1) plus one undocumented CLI spot (L-A1). Naming discipline is excellent (73 `*Cmd`, 72 `*Dto`, 49 `*Query`, no strays), the error-mapping chain (`DbErr → RepoError → ServiceError → GraphQL extension code`) is real and consistent, DataLoaders eliminate the obvious N+1s, and authorization is centralized through a shared access gate (`ensure_conversation_member_access` / `ensure_server_member`) applied at 29 call sites — so IDOR is mitigated by construction rather than per-resolver vigilance. Crypto is genuinely strong.

The findings cluster in three places:
1. **The account-recovery / login surface** — reset tokens are written to logs, there is no login rate limiting, and password reset neither revokes sessions nor is atomic.
2. **An SSRF hole** in the link-preview fetcher.
3. **The pagination contract** — a `BEFORE`-direction bug that appears to break message-history loading past the first page, with the same latent flaw copy-pasted across five hand-rolled paginators.

None of these is architectural rot; they're concrete, fixable defects on top of a sound foundation.

---

## Security findings

### High

**S-H1 — Password-reset tokens are written to application logs in plaintext.** `rim_infra_services/src/email.rs:13-19`, wired as the sole production sender at `provider.rs:139-145`. The only `EmailSender` in production wiring is `LogEmailSender`, which logs the raw token (`info!(... "PASSWORD RESET TOKEN ...: {}", token)`). There is no real email path, so this is default behavior in every deployment. Anyone with log read access (centralized logging, ops, backups) can request a reset for any username and read the token → full account takeover.
*Fix:* never log the raw token; gate `LogEmailSender` behind an explicit dev-only flag and ship a real sender, or at minimum log only a token id.

**S-H2 — SSRF in the link-preview fetcher via DNS.** `rim_infra_services/src/link_preview.rs:87-110` (`is_url_safe`), fetch at `:237-243`, triggered from `send_message` (`rim_api/src/mutation.rs:715`). `is_url_safe` rejects only URLs whose host is a **literal** private IP (plus `localhost`/`*.local`). A hostname that *resolves* to `169.254.169.254` (cloud metadata), `127.0.0.1`, or an internal `10./192.168.` address passes, and reqwest then fetches it server-side; the title/description/image are stored and returned to clients. The redirect guard has the same literal-IP-only weakness.
*Fix:* resolve the host and reject if **any** resolved address is private/loopback/link-local before connecting (validating DNS callback or a resolver+socket guard); apply the same check inside the redirect policy.

**S-H3 — No rate limiting or lockout on login.** `rim_infra_services/src/auth.rs:551-605`. `login` runs `find_by_username` + `verify_password` with no per-IP or per-account throttle (only `signup_open` and `preview_invite_token` have limiters). This permits unbounded password brute-force / credential stuffing. Secondary effect: every attempt against a known username triggers a 64 MiB / t=3 / p=4 Argon2 verify, so a login flood is also a memory/CPU exhaustion vector.
*Fix:* add a per-IP and per-username sliding-window limiter (reuse `RegistrationRateLimiter`) with exponential backoff / temporary lockout after N failures.

### Medium

**S-M1 — Password reset does not revoke existing sessions.** `rim_infra_services/src/password_reset.rs:83-121`. `reset_password` updates the hash and marks the token used but never calls `SessionRepository::revoke_all_for_user` (which exists but has **zero** production callers). An attacker with a stolen session retains access after the victim completes recovery — defeating the whole point of a reset.
*Fix:* call `revoke_all_for_user(record.user_id)` in the same transaction as the password update; invalidate other outstanding reset tokens too.

**S-M2 — `workspacePresence` query is an IDOR.** `rim_api/src/query.rs:815-828`. The resolver calls `require_actor` but never verifies membership of `workspace_id`; `PresenceManager::get_workspace_presence` is an unguarded map read. Any authenticated user can enumerate who is online (and `last_seen_at`) in any workspace by ID. `startTyping`/`heartbeat` *do* check membership — this query is the outlier.
*Fix:* call `ensure_server_member` before returning presence.

**S-M3 — Login user-enumeration timing oracle.** `rim_infra_services/src/auth.rs:554-586`. On unknown username, `login` returns `Unauthenticated` immediately without running Argon2; a known username pays the full ~tens-of-ms hash. The delta reliably distinguishes valid usernames. The in-code comment claiming timing indistinguishability is inaccurate — no dummy verify is performed. Same shape in `password_reset.rs:48-55`.
*Fix:* run a `verify_password` against a fixed dummy PHC hash on the user-not-found branch so both paths pay equal Argon2 cost.

**S-M4 — No rate limiting on `requestPasswordReset`.** `rim_infra_services/src/password_reset.rs:48-80`. Unbounded reset requests allow token-generation flooding and (once a real sender exists) email-bombing arbitrary users.
*Fix:* rate-limit by IP and target username.

**S-M5 — No storage-quota enforcement (authenticated storage-exhaustion DoS).** `rim_infra_services/src/storage.rs:97-105` hardcodes `get_quota_usage` to `used_bytes: 0`, and `handle_file_upload` performs no quota check. `max_workspace_storage_bytes` is configured but never enforced. Any authenticated user can upload unbounded 25 MB files until the disk fills.
*Fix:* track real usage and reject uploads over the workspace quota before writing.

### Low

- **S-L1 — `sendMessage` GraphQL path has no rate limit.** `rim_api/src/mutation.rs:689-718`. The command dispatcher enforces a `chat.send` limit, but the direct `sendMessage`/`replyToMessage` mutations bypass it — a member can flood a conversation and trigger unlimited link-preview fetches. Apply a per-actor send limiter in the service.
- **S-L2 — `myMentions` limit is unbounded.** `rim_api/src/query.rs:766-788` → `mention.rs:82` forwards the client `limit` with no clamp (siblings use `.min(100)`). `limit: 4000000000` drives a huge `SELECT ... LIMIT` + bulk fetch. Clamp it.
- **S-L3 — Link-preview body buffered without a hard streaming cap.** `link_preview.rs:246-256`. When the origin omits `Content-Length`, `response.bytes().await` buffers the full body before the size check — a memory-exhaustion vector reachable via S-H2. Stream with `chunk()` and abort past `MAX_BODY_SIZE`.
- **S-L4 — `Secure` cookie flag defaults off.** `graphql_support.rs:199-215`, `config.rs:188`. Cookies get `Secure` only with `RIM_FORCE_SECURE_COOKIES=true` or a trusted-proxy https signal. A deployment that forgets the flag ships session cookies over plaintext HTTP. Given the direct-HTTPS Tauri target, default it on and require explicit opt-out for local dev.
- **S-L5 — `reset_password` check-then-mark is not atomic.** `password_reset.rs:84-117`. The `used_at` check and `mark_used` are separate statements, so two concurrent requests with the same token can both succeed (benign here, but the bootstrap/invite paths correctly use atomic `try_mark_used`/`try_consume` — this one doesn't). Use a conditional `UPDATE ... WHERE used_at IS NULL`.
- **S-L6 — Signup reveals username-vs-email existence.** `auth.rs:318-335` returns distinct `Conflict "username already exists"` vs `EmailTaken`, enabling enumeration. Return a single generic error or document the tradeoff.
- **S-L7 — LIKE wildcard injection in message search.** `rim_infra_db/src/sea_orm_impl/message.rs:201,235` use `Column::Text.contains(query)`. SeaORM binds the value (no SQL injection) but does not escape `%`/`_`, so user input is interpreted as LIKE wildcards → broad/leading-wildcard matches and expensive scans (scoped to conversations the actor can already read). Escape `%`, `_`, `\`.
- **S-L8 — Transitive `rsa 0.9.10`** (via `sqlx-mysql`) carries RUSTSEC-2023-0071 (Marvin timing attack, no fix). The app uses PostgreSQL so the path is never exercised, but it compiles in. Disable the sqlx MySQL feature to drop it.

### Informational
- `/api/sdl` is unauthenticated and introspection is enabled (`server.rs:106`, `lib.rs:128-129`) — full schema disclosure to anonymous callers. Intentional for codegen; consider auth-gating in prod. GraphiQL is correctly gated behind `enable_graphiql` (default false).
- `requestPasswordReset`/`resetPassword` are **not** CSRF-exempt but `validate_csrf_header` requires a session cookie — so a logged-out user (the actual reset case) can't call them. Fail-closed, not a vuln, but a correctness gap that may tempt a future dev to weaken CSRF; fix the flow without broadening exemptions.
- `logout` is CSRF-exempt (`graphql_support.rs:56`) — enables forced-logout CSRF (denial only). Minor.
- Sessions have a 30-day TTL with no rotation or idle timeout (`request_auth.rs:42-53`). Consider idle expiry and rotation on privilege change.
- `.env` at repo root holds dev DB creds and a weak `RIM_BOOTSTRAP_SECRET=cafebabe0deadbeef`. It is correctly `.gitignore`d/`.dockerignore`d, so this only matters if it was ever committed before being ignored — confirm with `git log --all -- .env` and use a high-entropy secret in prod.
- `sea-orm 2.0.0-rc.38` is a pre-release in a security-relevant data layer — track pinning to a stable 2.0.

### Verified clean (traced, no issue)
- **Crypto:** Argon2id at 64 MiB/t=3/p=4 with per-hash salt and transparent legacy-param verification (`rim_auth/src/password.rs`); 256-bit CSPRNG session/CSRF/invite/reset tokens (`getrandom`), stored only as SHA-256 hashes; bootstrap secret compared with `subtle::ct_eq`.
- **CSRF:** HMAC-bound double-submit tokens keyed by the session token; enforcement parses the GraphQL AST to extract real mutation field names (defeats `operationName` spoofing) and **fails closed** on parse errors / unresolved fragments; applied to the multipart REST endpoints too.
- **Authorization / IDOR:** every query/mutation calls `require_actor`; the service layer enforces a 3-step membership gate with `NotFound`-first ordering that avoids leaking conversation existence to non-members; attachment downloads are membership-gated via `authorize_download`.
- **CORS:** single configured origin with credentials, or all cross-origin blocked when unset — no wildcard-with-credentials.
- **Injection / traversal:** no string-interpolated SQL (raw statements use parameterized `from_sql_and_values`); `sanitize_filename` + `LocalFileStorage::validate_path` reject `..`/null and canonicalize against root; no `unsafe` blocks anywhere in the workspace.
- **WebSocket** rejects anonymous users at the transport level, independent of resolver checks.
- **GraphQL DoS:** `limit_depth(10)` + `limit_complexity(200)` on the runtime schema; most list resolvers clamp page size.

---

## Architecture & implementation-quality findings

### High

**A-H1 — `BEFORE`-direction pagination breaks older-history loading; frontend and backend disagree on cursor direction.** Backend (`rim_infra_db/src/sea_orm_impl/message.rs:108-187`): for the DESC endpoint, `PageDir::Before` means `sent_at > cursor` (toward *newer*) and `next` for `(Desc, Before)` is the newest cursor of the page. Frontend (`rim-ui/src/features/messages/api/useMessages.ts:27-46`) opens the list with `dir: "BEFORE"` and follows `page.next` expecting *older* messages. Trace: initial `BEFORE` (no cursor) returns newest 50, `next` = newest cursor → `fetchNextPage` sends `BEFORE` + that cursor → filter `sent_at > newest` → **empty page**. Infinite scroll-up dead-ends after 50 messages. A **second, independent** defect in the same block: for `Before`-with-cursor the query keeps the display `ORDER BY` + `LIMIT n`, so it returns the *far end* of the matching range, skipping everything between the cursor and the boundary. This block is copy-pasted into `message_thread.rs:196-270`, `conversation.rs:162-330` (×2), and `incident.rs:141-200` — all share the flaw. No e2e test walks the resolver→repo cursor round-trip, so repo tests (which paginate with `After`) never exercise the frontend's actual call pattern.
*Fix:* (1) invert `ORDER BY` for the anti-direction, take `n`, reverse in memory; (2) extract **one** generic `(ts, id)` paginator instead of five copies; (3) add an e2e test walking a 3-page conversation both directions through GraphQL; (4) align the rim-ui call sites (its idiom maps to `After` on DESC endpoints). **Recommend runtime verification** — this reads as a live production bug.

### Medium

**A-M1 — `mockall` is compiled into production binaries.** `rim_server`, `rim_api`, `rim_infra_services` all declare `rim_services = { features = ["mocks"] }` under `[dependencies]`. Cargo feature unification means every release build carries the mock services.
*Fix:* keep the plain dep and re-declare with `features = ["mocks"]` under `[dev-dependencies]` only.

**A-M2 — Thread-creation race recovery is broken inside a real transaction.** `rim_infra_services/src/message.rs:517-556` (`reply_to_message`): inside a write transaction, a `Conflict` from `create_thread_for_root` is caught and the code re-queries `get_by_root_message` on the same transaction. In Postgres a unique violation aborts the transaction, so the follow-up fails with `25P02 current transaction is aborted` — the exact race this code exists to survive will 500. Unit tests pass because `db: None` disables the transaction.
*Fix:* wrap the insert in a savepoint (nested `DbHandle::transaction` already does savepoints) or retry the whole transaction on conflict; add a concurrent-reply integration test.

**A-M3 — Transactionality is optional and implicit.** Services hold `db: Option<DatabaseConnection>` (`message.rs:36`, `provider.rs`); `begin_write()` silently runs non-transactionally when `None`. Production always passes `Some`, but unit tests exercise a different atomicity regime than prod (masking A-M2) and a future wiring mistake degrades silently. It also threads a raw `sea_orm::DatabaseConnection` through the business layer.
*Fix:* make the unit-of-work mandatory via a small `TxProvider` trait in `rim_services` (no-op test impl), so the business layer stops naming SeaORM types.

**A-M4 — Over-broad DB error classification maps almost everything to `Conflict`.** `rim_infra_db/src/error.rs:21-38`: any `E::Database` with a SQLSTATE code that isn't a recognized constraint still becomes `Conflict`. Since every Postgres error carries a code, serialization failures, deadlocks, NOT NULL violations, and disk-full all surface as GraphQL `CONFLICT` with `"database error code: 40001"` — misleading, and it leaks constraint names/SQLSTATEs into client-visible `Conflict(msg)`.
*Fix:* match SQLSTATE classes (`23xxx` → Conflict, `40xxx` → retryable/Unavailable, else Internal) and keep detail server-side.

**A-M5 — `unwrap`/`expect` panics in production paths** (violates the repo's own "no unwrap/expect in library code" rule). Confirmed non-test sites: `notification_preference.rs:59` `.expect("just saved")` (a concurrent delete between update and refetch panics a request thread); `message_attachment.rs:64` `uuid_val.get_timestamp().unwrap()` (panics on any non-UUIDv7 id, e.g. imported rows); `mutation.rs:576` `input.workspace_invite_token.unwrap()` (currently branch-guarded but fragile). The `getrandom` panic and `LazyLock` regex `expect`s are documented/defensible.
*Fix:* propagate as errors.

**A-M6 — Subscription events bypass the GraphQL type system.** `rim_api/src/subscription.rs:181` returns `Stream<Item = String>` of serde-JSON `WorkspaceEvent`s. The `/api/sdl`→codegen contract that CLAUDE.md calls CRITICAL does not cover the most change-prone surface — real-time events are an undocumented parallel serde contract (exactly the schema-drift class that already bit the plugin system). `user_can_see_event` also compares `conversation_type == "Channel"` stringly and allocates `id.to_string()` per member per event per subscriber.
*Fix:* a typed GraphQL union for events (async-graphql supports subscription unions), or at minimum a published JSON schema; compare typed IDs/enums.

**A-M7 — e2e coverage doesn't match the risk profile.** `rim_api/tests/` contains only `registration_resolvers_e2e.rs`; the messaging core (send/reply/paginate through resolver+service+repo) has no through-the-stack test — which is exactly where A-H1 lives. Unit tests themselves are meaningful (mockall repo mocks + real service impls), not padding.
*Fix:* add one messaging e2e walking pagination/threads/reactions under the existing `e2e-tests` feature job.

**A-M8 — `AppRepos` wiring duplicated.** `rim_server/src/app.rs:63-143` hand-builds all ~30 repos; `rim_infra_services/src/repos.rs:393-505` (`AppRepos::with_handle`) builds the identical set. Adding a repo requires touching both; drift is silent.
*Fix:* have `app.rs` call `AppRepos::with_handle(&db)`.

### Low
- **A-L1** — Undocumented third layering exception: `rim_server/src/bootstrap_cmd.rs:27-28` imports `rim_infra_db` directly (CLI bootstrap). Defensible wiring, but FL1 names only `app.rs` — document it.
- **A-L2** — Business logic in resolvers: `start_typing` (`mutation.rs:1307-1360`) and `signup_with_invite` (`mutation.rs:513-642`) own orchestration/policy at the transport layer; push behind service methods.
- **A-L3** — Dead "fail closed" comment: `mutation.rs:601-603` discards the `Result` of `ctx.data::<SignupRateLimiterToken>()` while claiming fail-closed. Use `?`.
- **A-L4** — Monolithic resolver files: `mutation.rs` 2,412 / `query.rs` 1,369 / `types.rs` 2,793 lines, single structs. Split by domain via `MergedObject`/`MergedSubscription`.
- **A-L5** — Copy-paste UoW helpers (`begin_write`/`commit_if_present`) duplicated across message/reaction/auth/incident/server services — extract once (pairs with A-M3).
- **A-L6** — Limit-clamp inconsistency: messages resolver clamps to 100, service to 200, audit to 200. Pick one constant, one layer.
- **A-L7** — `ReactionLoader` assumes a single actor per batch (`loaders.rs:86-88`, `keys[0].actor_user_id`) — latent bug if loader cache scope changes; loader errors also bypass `map_service_error` (no `code` extension).
- **A-L8** — Presence/typing edge cases: `DisconnectGuardStream::drop` marks a user offline when *one* of several tabs closes (`subscription.rs:32-37`); heartbeat rate-limit can drop a legitimate Online→Away within 10s; `TypingTracker::should_broadcast` has a benign get-then-insert TOCTOU. Concurrency is otherwise disciplined (DashMap refs dropped before emits, no blocking calls in async paths).
- **A-L9** — N+1 in mention validation: `process_mentions` calls `is_member` per mentioned user in a loop (`message.rs:92-104`). Batch it. (List paths are properly batched — good.)
- **A-L10** — Mentions written post-commit best-effort (`message.rs:444-451`): the `UserMentioned` event and mention rows can be lost on crash while the message survives — acceptable, but state it's intentional.
- **A-L11** — Doc drift in `actual_state.md`: claims private channels (model has only `Direct|Group|Channel`) and a sensitivity taxonomy that doesn't match code (`Safe|Sensitive|Mutating|Restricted`). Also `rim_services/plugin_host` (~1,290 LOC) is dead code (as the doc admits). Note: if private channels ever land, `user_can_see_event`'s "Channel ⇒ visible to all workspace members" branch becomes a leak — add a guard comment now.
- **A-L12** — `builtin_commands()` rebuilds the full definition Vec on every lookup (`dispatch.rs:31-35`). `LazyLock` it.

---

## What's done well (worth preserving)
- Compiler-enforced layer boundaries that actually hold; scoped FL1/FL2 exceptions respected.
- `DbHandle`/`TransactionScope` — a clean shared-transaction design with rollback-on-drop and savepoint support.
- Migration hygiene: 49 timestamped files with real `down()`, dedicated FK-policy/index/hardening migrations. Entity mappers are exhaustive struct literals, so schema/entity drift is a compile error.
- Error taxonomy with client-stable extension codes (incl. `RATE_LIMITED.retryAfterSeconds`) and no source leakage on `Internal`.
- Substantive test suite (~800 tests, mockall at the right seam, feature-gated DB/e2e tiers, a real 95% lcov gate). *Honesty note:* the CI `cargo audit` step is `continue-on-error` (warning-only), softer than CLAUDE.md implies — and it currently can't parse the advisory DB, so the dependency gate is effectively not running. Worth fixing so it's real.

---

## Suggested remediation order
1. **S-H1** (reset tokens in logs) — trivial, highest exposure.
2. **S-H2** (link-preview SSRF).
3. **S-H3 / S-M1 / S-M3 / S-M4** (login + reset hardening as a set).
4. **A-H1** (pagination — fix with the frontend and add the e2e test).
5. **S-M2** (presence IDOR), **S-M5** (storage quota).
6. **A-M1 / A-M2 / A-M3** (mockall in prod, then transaction correctness together).
7. Remaining Mediums and the duplication cleanups opportunistically.
