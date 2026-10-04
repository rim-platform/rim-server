# Registration Redesign — Post-Review Hardening Plan

**Author:** Nick Fury (CTO)
**Date:** 2026-04-28
**Status:** review complete; fixups pending in a fresh session
**Companion specs (sealed once these fixups land):**
- `docs/design/registration-redesign-2026-04-28.md` (backend, with §13 amendment)
- `../rim-ui/docs/registration-redesign-frontend-2026-04-28.md` (frontend, with §13b amendment)

---

## Context

Registration redesign landed in 4 commits across both repos:

1. **rim-server** — multi-tenant onboarding redesign (bootstrap CLI, instance invites, signup mutations, registration mode boot wiring)
2. **rim-server** — R2/R3 cleanup + 3 resolver-level e2e tests
3. **rim-server** — Amendment 2026-04-28b (`email` + `displayName` on signup, migration with synthetic backfill, `EmailTaken` error)
4. **rim-ui** — frontend implementation of (1) + Amendment 13b (`email` + `displayName` JoinForm fields, `EMAIL_TAKEN` error mapping)

Hawkeye + Black Widow ran review pass on the cumulative diff. Hawkeye: clean (no must-fix). Black Widow: 1 CRITICAL + 4 HIGH + 7 MEDIUM + 5 LOW.

This memo captures the agreed fixup plan so we can resume in a fresh session without re-deriving it.

---

## Workstream A — rim-server (security-critical, must land before sealing specs)

| ID | File:Line | Issue | Fix |
|---|---|---|---|
| **C1** | `rim_infra_db/src/sea_orm_impl/bootstrap_token.rs:103-117`, `rim_infra_services/src/auth.rs:873-924` | Bootstrap token double-spend race. `mark_used` does check-then-update without atomic guarantees; two concurrent requests with same raw token both pass the unused-check. Single-use guarantee broken. | Change to `UPDATE bootstrap_token SET used_at = now(), consumed_by_user_id = $2 WHERE id = $1 AND used_at IS NULL AND revoked_at IS NULL`. Treat `rows_affected == 0` as `InvalidToken`. Run inside the signup txn; bail (rollback) when 0. Mirror the existing `instance_invite::try_consume` pattern. |
| **H1** | `rim_server/src/server.rs:170-175` | Rate limiter trusts `X-Forwarded-For` even with no proxy in front. Defeats both 30/5min preview limit and 20/10min open-signup limit. | Read `peer_addr` by default; honor `X-Forwarded-For` only when `RIM_TRUST_FORWARDED_HEADERS=true` is set. Document the operator requirement. |
| **H2** | `rim_auth/src/password.rs:16` | `Argon2::default()` yields m=19MiB, t=2, p=1 — below Jakub's stated production target (m=64MiB, t=3, p=4). | `Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::new(64*1024, 3, 4, None)?)` for both `hash_password` and `verify_password`. PHC-string verification handles old hashes transparently. |
| **H3** | `rim_server/src/graphql_support.rs:189-194,210,220,234,242` | `Secure` cookie flag depends solely on `X-Forwarded-Proto`. Direct-HTTPS deployments (Tauri WebView, Caddy/Traefik without the header) lose `Secure`. | Default to `req.connection_info().scheme() == "https"`; `RIM_FORCE_SECURE_COOKIES=true` env override; only consult `X-Forwarded-Proto` when proxy-trust is enabled (see H1). |
| **H4** | `rim_infra_services/src/auth.rs:927,1022,1158,1217` | Audit-trail gaps: missing signup success/failure (per mode), `email_taken_attempt`, login success/failure, `signup_rate_limited`, `instance_invite_revoked`, `bootstrap_token_revoked`. Operator can't detect attacks. | Add `tracing::info!(target="audit", event=…, …)` at success + each typed-error return in `signup_with_bootstrap`, `signup_with_instance_invite`, `signup_open`, `accept_invite`, `login`, `revoke_*`. Include `event="email_taken_attempt"` inside `create_user_in_txn` when `EmailTaken` branch fires. |
| **M3** | `rim_infra_services/src/auth.rs:387` | Bootstrap secret compared with `!=` (timing side channel on fresh deploy). | Replace with `subtle::ConstantTimeEq::ct_eq`. |
| **M4** | `rim_infra_db/src/sea_orm_impl/instance_invite.rs:105-123` | `try_consume` only checks `use_count < max_uses`; concurrent admin revoke between pre-check and consume lets one signup slip past revocation. | Add `RevokedAt.is_null()` and `ExpiresAt.gt(current_timestamp())` to the WHERE clause. |
| **M5** | `rim_infra_services/src/registration_rate_limiter.rs:118-140` | IPv6 prefix rotation defeats per-IP rate limit. DashMap key is full IP; attacker on /64 rotates suffix freely across 2^64 addresses. | Normalize IPv6 to /56 (or /64) before keying DashMap. IPv4 stays /32. |
| **M6** | `rim_infra_services/src/registration_rate_limiter.rs:45,57` | DashMap is unbounded — memory DoS via H1+M5. | Cap entries at 100k; evict LRU on overflow. |
| Hawkeye SF1 | `rim_infra_services/src/registration_rate_limiter.rs:1-37` | Dead code: `PreviewInviteRateLimiter` / `SignupRateLimiter` newtype wrappers. Nothing constructs or consumes them post-R2. | Delete. The marker tokens in `rim_services::auth::dto:163-179` are the actual proof-of-wiring artifacts; the wrappers were a pre-R2 leftover. |
| Hawkeye Nit1 | `rim_api/src/query.rs:1314` | Stale comment claims "Fail-closed: preview rate limiter MUST be wired into context (§4.7 boot invariant)". R2 made the limiter internal to `DefaultAuthService`. | Update wording: limiter is internal; context just carries a marker token. |

**Tests:** every fix gets unit tests. C1 specifically needs a concurrent-double-spend test (use a tokio mutex + barrier or `tokio::join!` two `signup_with_bootstrap` calls and assert exactly one succeeds).

---

## Workstream B — rim-ui (frontend hardening)

| ID | File | Issue | Fix |
|---|---|---|---|
| **M1** | `src-tauri/tauri.conf.json:22` | Tauri CSP `connect-src http://localhost:*` overly permissive in release. Lets any local web server be addressable from inside the auth-bearing WebView. | Split CSP: dev keeps current; release pins `connect-src` to `VITE_GRAPHQL_ENDPOINT` host (or env-driven URL). Document the release build step. |
| **M2** | `Dockerfile:44-48` | Web build (nginx) sets `X-Frame-Options`, `X-Content-Type-Options`, `Referrer-Policy` but **no** `Content-Security-Policy`. No XSS defense-in-depth. | Add `add_header Content-Security-Policy "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self' <api-host> wss://<api-host>; frame-ancestors 'none'" always;`. Verify against live asset loads. |
| **L2** | `Dockerfile:47` | `X-XSS-Protection: 1; mode=block` is deprecated and historically harmful. | Set to `"0"` and rely on CSP (M2). |
| **L3** | `JoinPage.tsx` (and any token-bearing routes) | Token in URL is leaked via Referer to third parties under current `strict-origin-when-cross-origin`. Origin only, not full path — but defense-in-depth still wants no-referrer on token routes. | Add `<meta name="referrer" content="no-referrer">` on `JoinPage` and `ResetPasswordPage`. |
| **L4** | `src/features/auth/components/LoginForm.tsx:63` | Outdated `?return_to=` comment. Implementation uses `location.state.returnTo` (router state, not query param). Comment misleads future contributors into adding an open-redirect vector. | Drop the comment, or replace with explicit safety note: "return target is router state only; do not parse from query string without server-side allowlist." |
| Hawkeye SF2 | `src/features/auth/components/JoinForm.tsx` | No client-side guard for displayName 1–80 char range. User pastes 81 chars, eats a round-trip + generic validation error. | Mirror email-validation pattern: short inline check before submit. Locked copy not in spec — use a sensible default (e.g., "Display name must be at most 80 characters"). |

**Tests:** add component tests for the displayName length guard and the JoinPage no-referrer meta. CSP changes tested manually (live build).

---

## Workstream C — spec hygiene (do before sealing)

| ID | File | Issue | Fix |
|---|---|---|---|
| Hawkeye SD1 | `rim-server/docs/design/registration-redesign-2026-04-28.md` §13.7 | Spec still describes `AcceptInviteCmd` gaining `email` + `display_name` fields. Implementation deliberately scoped that down per Q2 ruling — `acceptInvite` is untouched. | Mark §13.7 superseded by Q2 ruling. Implementation note: legacy `accept_invite` path uses `<username>@local.invalid` synthetic placeholder. |
| Hawkeye SD2 | `rim-ui/docs/registration-redesign-frontend-2026-04-28.md` §8.2 | Spec describes `query { serverConfig { registrationMode } }`. Backend shipped a top-level `query { registrationMode }` direct on Query. Frontend `useServerConfig` adapted (queries `registrationMode` directly, wraps the result in a `ServerConfig` shape on the JS side), but the spec text was never updated. | Update §8.2 to match the simpler GraphQL surface that actually shipped. |

After C is done: seal both specs with a status header and move to `docs/decisions/` (or equivalent) in each repo. See "Sealing" section below.

---

## Deferred — track but don't fix in this round

- **M7** — `EMAIL_TAKEN` is a public oracle in `RIM_OPEN_REGISTRATION=true`. Open mode is opt-in and rate-limited; closing this requires a "send email to existing user, return generic success" flow that needs a real email-sending infra we don't have. Track as backlog hardening for when email infra lands.
- **L1** — Bootstrap CLI prints raw token to stdout. Acknowledged design ("operator pipes it directly"). Optional `--quiet` mode would harden CI usage. Tracking only.
- **L5** — `signupWithInvite` workspace branch silently uses synthetic email. Functional quirk per spec; UX inconsistency (EMAIL_TAKEN won't fire on workspace branch even though form collects email). Re-scope when we revisit per-workspace-invite UX.
- **I5** — 7 pre-existing transitive `cargo audit` findings (bytes 1.11.0, rkyv 0.7.45, rsa 0.9.9 via dead sqlx-mysql path, rustls-webpki 0.103.8 — 4 CRL/name-constraint bugs). None introduced by registration redesign. Track for next dependency-bump PR.
- **rim_infra_db/tests/support/mod.rs** truncate list missing `bootstrap_token` and `instance_invite`. Pre-existing gap surfaced by hulk during Workstream A's e2e test work. Will cause stale rows in `--features db-tests` runs. Small follow-up.

---

## Dispatch plan (resume here)

1. **Parallel dispatch:** hulk on Workstream A (rim-server), spider-man on Workstream B (rim-ui). Disjoint repos; no contention.
2. After each reports green: Jakub commits each repo. Suggested commit shape — two commits per repo (one for security-critical, one for cleanup/M-tier) but a single "registration redesign hardening" commit per repo is fine if Jakub prefers fewer commits.
3. **Workstream C — spec hygiene:** dispatch iron-man + vision in parallel after A and B have landed. Tiny edits.
4. **Sealing:** add status header to both specs:
   ```
   > **Status:** Implemented 2026-04-28, hardened <date>. See commits <hashes>.
   > **Do not edit** — historical record. For changes, write a new ADR.
   ```
   Move both files:
   - `rim-server/docs/design/registration-redesign-2026-04-28.md` → `rim-server/docs/decisions/`
   - `rim-ui/docs/registration-redesign-frontend-2026-04-28.md` → `rim-ui/docs/decisions/`
5. Create a `docs/decisions/README.md` in each repo if it doesn't exist, explaining the convention (sealed historical specs, ADR-style).

---

## Black Widow informational findings (already verified sound — no action)

- **I1** — Token entropy + CSPRNG sourcing correct. `getrandom::fill` (OS CSPRNG), 256-bit entropy, base64url, prefix-tagged. SHA-256 hash-only storage, unique-indexed.
- **I2** — Email lookup SQL is parameterized via `Expr::cust_with_values`, not interpolated. Migration `execute_unprepared` calls contain only static SQL.
- **I3** — CSRF design is appropriate. HMAC-bound to session token via `rim_auth::csrf`, constant-time verify, enforced on mutations. Exemption list (bootstrap, login, acceptInvite, logout, signupWithInvite) is justified — those mutations target unauthenticated callers with no existing session.
- **I4** — No password values in logs anywhere in the auth path. Username logged (acceptable). Email logged only by bootstrap CLI on stderr (intentional audit correlation).
- **I6** — Tauri command surface unchanged; no new attack surface from `rim-ui/src-tauri/`.

---

## Final state required before sealing

- [ ] Workstream A landed + committed (rim-server)
- [ ] Workstream B landed + committed (rim-ui)
- [ ] Workstream C spec edits done
- [ ] `cargo test --workspace -- --test-threads=1` green
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `npm run build` + `npm test` green (rim-ui)
- [ ] Hawkeye + Black Widow re-review on the hardening commits (spot-check, not full re-audit)
- [ ] Specs sealed and moved
- [ ] Backlog items (M7, L1, L5, infra_db truncate, cargo audit) tracked in a separate followups doc or issue tracker

---

## Cold-start instructions for the next session

1. Read this memo first.
2. Read `docs/design/registration-redesign-2026-04-28.md` (backend spec, with §13 amendment + R1/R2/R3 rulings) and `../rim-ui/docs/registration-redesign-frontend-2026-04-28.md` (frontend spec, with §13b amendment).
3. Dispatch hulk + spider-man in parallel on Workstreams A and B.
4. Default to "minimal diff" — these are surgical fixups, not redesigns.
5. Before sealing the specs, verify Workstream C edits are accurate against the live SDL and live React code, not just against the specs from memory.
