# Registration Redesign — Follow-ups + Resume Prompt

**Author:** Nick Fury (CTO)
**Date:** 2026-04-28
**Status:** active — backlog to pick up in a future session

## Context

The registration redesign + post-review hardening pass is complete and sealed. Both specs moved to `docs/decisions/`. This file tracks what's deferred and what emerged during implementation that wasn't worth blocking the seal.

## Sealed specs

- `rim-server/docs/decisions/registration-redesign-2026-04-28.md`
- `rim-ui/docs/decisions/registration-redesign-frontend-2026-04-28.md`

## Commits in this session

### rim-server (`feature/10-lets-start-over`)

- `9538c66` — security hardening + rate limits and cleanup (Workstream A: C1 atomic bootstrap-token claim, H1-H4, M3-M6, SF1, Nit1; migration Vec restoration; migration inline-`.index()` Postgres fix; bootstrap CLI URL shape; BOOTSTRAP enum split (BE half); session txn boundary fix + new live-DB regression tests).
- `c1e3846` — security hardening + cleanups (Workstream C: spec corrections + sealing prep).

### rim-ui (`feature/initial-code`)

- `0b335af` — security hardening + rate limits and cleanup (Workstream B: M1-M2 CSP, L2-L4, SF2 displayName guard; BOOTSTRAP variant routing in JoinForm + JoinAuthenticated + tests).
- `8d6ac88` — security hardening + cleanups (Workstream C: spec corrections + sealing prep).

## Optional final step (deferred)

- **Hawkeye + Black Widow spot-check** on the hardening commits. Not a full re-audit. Listed as optional in the original memo. Worth doing before merge to main if not yet done.

## Backlog from the original hardening memo (deferred)

- **M7** — `EMAIL_TAKEN` is a public oracle when `RIM_OPEN_REGISTRATION=true`. Open mode is opt-in and rate-limited. Closing this requires a "send email to existing user, return generic success" flow that needs real email-sending infra we don't have. Track for when email infra lands.
- **L1** — Bootstrap CLI prints raw token to stdout. Acknowledged design ("operator pipes it directly"). Optional `--quiet` mode would harden CI usage. Tracking only.
- **L5** — `signupWithInvite` workspace-invite arm silently discards user-supplied email/displayName (Q2 ruling: don't touch `accept_invite`; user lands with `<username>@local.invalid` synthetic placeholder). Re-scope when we revisit per-workspace-invite UX.
- **I5** — 7 pre-existing transitive `cargo audit` findings (bytes 1.11.0, rkyv 0.7.45, rsa 0.9.9 via dead sqlx-mysql path, rustls-webpki 0.103.8 — 4 CRL/name-constraint bugs). None introduced by registration redesign. Track for next dependency-bump PR.
- **`rim_infra_db/tests/support/mod.rs`** truncate list missing `bootstrap_token` and `instance_invite`. Hulk addressed the parallel gap in `rim_infra_services/tests/support/mod.rs` while writing the C1/session-txn regression tests; the matching `rim_infra_db` truncate list remains pre-existing-gap. ~5-min fix.

## Backlog that emerged during this session

- **`JoinPage.tsx:122` cast hardening** — `variant={preview.tokenType as InviteTokenType}` is sound today (the enum has only the variants the form handles), but a 4th enum variant added later would silently pass TS and cause `buildInput()` to fall through to the OPEN branch (no token field set), producing a runtime no-op signup. Suggested mitigations: (a) replace cast with an exhaustive switch in `JoinPage` that narrows the variant explicitly, or (b) have `buildInput()` throw on unknown variant rather than silently falling through to OPEN. Spider-man flagged this during BOOTSTRAP routing work.
- **Atomic revoke methods** — `revoke` in `SeaOrmBootstrapTokenRepository` and `SeaOrmInstanceInviteRepository` does read-then-update (two round-trips). Could be single atomic `UPDATE ... SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL`. Mirror the C1 / M4 hardening pattern. Hulk flagged this during Workstream A. Concurrent-revoke test alongside.
- **`list_active` pagination cap** — neither `bootstrap_token::list_active` nor `instance_invite::list_active` has a cursor or hard cap. Operator with thousands of active invites would get unbounded responses. Add cursor pagination per the project's `(timestamp, id)` convention.
- **Frontend spec §10 title cosmetic** — "Open questions for Jakub" header with zero questions inside (vision flagged). Pure polish; if doing a future spec polish round, consider rename to "Resolved questions" or fold into a closing notes section. Sealed spec, so this would be a new ADR-style note rather than an in-place edit.

## Resume prompt for next session

Copy-paste the block below into a fresh Fury session:

---

Hey Fury, picking up the registration-redesign backlog. The redesign + hardening pass is complete and sealed. The full punch list is at:

`/home/kuba/RustroverProjects/rim-server/docs/followups/registration-redesign-2026-04-28-followups.md`

Read first:
1. The followups memo above.
2. `rim-server/docs/decisions/registration-redesign-2026-04-28.md` (sealed; do not edit).
3. `rim-ui/docs/decisions/registration-redesign-frontend-2026-04-28.md` (sealed; do not edit).

Suggested ordering by impact/effort (you decide what to pick up first):

1. **`rim_infra_db` truncate list** (~5 min) — add `bootstrap_token` and `instance_invite` to the truncate list in `rim_infra_db/tests/support/mod.rs`. Pre-existing gap from before the redesign; the parallel gap in `rim_infra_services/tests/support/mod.rs` was already closed during the session that wrote this memo.
2. **Atomic revoke methods** (~30 min, hulk) — fold `revoke` on `SeaOrmBootstrapTokenRepository` and `SeaOrmInstanceInviteRepository` into a single atomic `UPDATE` with `revoked_at IS NULL` guard. Mirror the C1 / M4 pattern. Concurrent-revoke unit test.
3. **`JoinPage.tsx` exhaustive variant** (~15 min, spider-man) — replace the `as InviteTokenType` cast in `rim-ui/src/features/auth/components/JoinPage.tsx:122` with an exhaustive switch that narrows the variant; or have `JoinForm.buildInput()` throw on unknown variant. Either approach turns a future fall-through bug into a compile-time or test-time error.
4. **`list_active` pagination** (1-2 hr, iron-man + hulk) — design + add cursor pagination to `bootstrap_token::list_active` and `instance_invite::list_active`. Follow project `(timestamp, id)` cursor convention. Schema impact only on the resolver / DTO, not the DB.
5. **Optional Hawkeye + Black Widow spot-check** on the four hardening commits if not done before merge.

Other deferred items (M7, L1, L5, I5) require either external infra or a UX revisit — not pickable as standalone tasks.

Hard rules unchanged:
- NEVER git commit / push / merge — Jakub commits manually.
- Every new function gets tests AND documentation.
- For new migrations: `sea migrate generate <name>` (never hand-create).
- For entity regeneration: `sea generate entity -l -o /tmp/rim-entities`, manually merge to `entity/src/` (entity/src/ has hand-written mappers that get clobbered if you generate directly).
- Stay strictly in scope.
- If a finding turns out wrong on closer inspection, push back — don't grind through it.

Local rim-server may be running on :8080; check before assuming.
