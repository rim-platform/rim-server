# Audit Log Access Matrix

**Phase:** 6 (P6-FU4)
**Date:** 2026-04-27
**Author:** Pepper Potts (documentation)
**Security owner:** Natasha Romanoff (Black Widow, CSO)
**Related tickets:** #23 P6-H2, #29 P6-FU4, #30 P6-FU5, #31 P6-FU6

This document is the canonical reference for which actor can read which audit
records under which conditions. Future contributors touching
`rim_infra_services/src/audit/mod.rs` MUST read this before modifying the
`list_entries` gate. The three bypasses closed by P6 are documented explicitly
in section 6 so they are not re-introduced.

---

## 1. Actors

RIM does not have first-class "Owner / Admin / Member / Guest" role objects
surfaced to the audit-read gate. The gate is capability-based: whether an actor
may read audit records depends on which capabilities they hold, not on a role
label. Role labels (if any) are assigned by the workspace and determine which
capabilities are granted; the service layer sees only capabilities.

The relevant actors for audit reads:

| Actor type | Typical capability grants | Notes |
|---|---|---|
| Workspace member (any role with `audit.read`) | `audit.read`, `audit.read.self` | Full workspace dump for the scoped workspace |
| Any authenticated user | `audit.read.self` | Own audit history only, no workspace scope required |
| Plugin | Per-plugin grant, never `is_self_scoped=true` | Plugins cannot self-scope; see `rim_infra_services/src/capability.rs:register_plugin_capabilities` |
| System / server-internal | `write_entry` path — no capability check | Write path is server-internal only; see `rim_services/src/audit/service.rs:13` |

Plugins declared in the registry have `is_self_scoped` forced to `false` by
`register_plugin_capabilities`. A plugin manifest cannot claim workspace-gate
bypass. See `rim_infra_services/src/capability.rs:293-308`.

---

## 2. Resource scopes

The `list_entries` gate branches on two optional filter fields supplied by the
caller: `actor_user_id` and `server_id`. Their combination determines which
resource scope is being requested:

| Scope label | `actor_user_id` | `server_id` | Description |
|---|---|---|---|
| Self-read | `Some(caller)` | `None` or `Some(W)` | Caller reads their own history |
| Workspace read | `None` | `Some(W)` | Caller reads all activity in workspace W |
| Cross-user read | `Some(other)` | `Some(W)` | Caller reads workspace W activity (P6-FU6: user filter stripped) |
| Forbidden — cross-user unscoped | `Some(other)` | `None` | Unconditionally denied |
| Forbidden — unscoped | `None` | `None` | Unconditionally denied |

The "Cross-user read" case deserves extra attention: the caller is authorized to
read workspace W, but the targeted `actor_user_id` filter is STRIPPED from the
DB query (P6-FU6, ticket #31). The response is a full workspace dump, not a
targeted user timeline. This prevents an adversary with `audit.read` from
enumerating whether a specific user ever acted in a workspace. See section 6 for
details.

---

## 3. Required capabilities

Two capabilities gate audit reads. Both are seeded at startup by
`builtin_capabilities()` in `rim_infra_services/src/capability.rs:44-89`.

| Capability | `is_self_scoped` | Required for |
|---|---|---|
| `audit.read` | `false` (workspace-scoped) | Workspace read; cross-user read |
| `audit.read.self` | `true` (self-scoped) | Self-read (no workspace membership required) |

`is_self_scoped` is stored in the `capability_registry` DB table, not hardcoded
in the check path. The service reads the DB row and enforces scope accordingly
(P6-FU5, ticket #30). A workspace-scoped capability called without a
`workspace_id` returns `PermissionDenied` before the membership check runs —
passing `None` can no longer silently bypass membership.

**Capability check path:**

```
rim_infra_services/src/capability.rs:147  DefaultCapabilityService::check()
  step 1: reject empty/whitespace key (Validation)
  step 2: registry lookup — unknown or retired key → PermissionDenied (strict default-deny, C1)
  step 2b: workspace-scoped cap + workspace_id=None → PermissionDenied (P6-FU5, #30)
  step 3: membership_holds(actor, workspace_id) → PermissionDenied if not member
```

**Write path** (`write_entry`) has no capability check. It is called only from
server-internal code paths (command executor, macro executor). It must never be
exposed to external callers directly.

---

## 4. Filters that MUST be applied at the service layer

The GraphQL resolver (`rim_api/src/query.rs:1203`) passes client-supplied filter
values through to the service. The service is the only trust boundary; nothing
the client sends is trusted to gate access.

### 4.1 Workspace membership (server_id REQUIRED for non-self reads)

Closed by ticket #23 P6-H2.

Before this fix, a caller could supply `actor_user_id=<victim>` with
`server_id=None` and read the victim's history without any workspace check. The
service now evaluates `(actor_user_id, server_id)` together:

- `actor_user_id=Some(other)` with `server_id=None` is unconditionally
  `PermissionDenied` — no capability check fires.
- All non-self paths require a `server_id`; the capability check for `audit.read`
  passes `workspace_id=Some(W)` to `CapabilityService::check`, which runs the
  membership leg.

See `rim_infra_services/src/audit/mod.rs:91-144` for the full match arm
implementation.

### 4.2 Cross-user filter stripped at service layer (P6-FU6, ticket #31)

When the caller supplies `actor_user_id=Some(other)` with `server_id=Some(W)`,
the service:

1. Checks `audit.read` in W (membership confirmed).
2. Emits a `cross_user_filter_stripped` warn-level audit event.
3. **Sets `actor_user_id=None` in the repo query** before calling the DB.

The DB therefore returns the full workspace dump for W, not a row-set filtered
to the targeted user. The response the caller sees contains activity from all
workspace members, making it impossible to determine whether a specific user
ever acted in W.

See `rim_infra_services/src/audit/mod.rs:112-126` and the stripping assignment
at line 147.

### 4.3 `capability_service.check` with `membership_holds(None)` silent bypass (P6-FU5, ticket #30)

Closed by ticket #30 P6-FU5.

Before this fix, `membership_holds(actor, None)` returned `Ok(true)`,
short-circuiting the membership check for any capability called without a
workspace ID. This allowed a workspace-scoped cap to pass the membership leg
when called without a workspace context.

The fix moves the workspace-scope gate to step 2b in `CapabilityService::check`
(before `membership_holds` is called). If `is_self_scoped=false` and
`workspace_id=None`, the call returns `PermissionDenied` immediately.
`membership_holds(actor, None)` is now only reached for self-scoped caps, where
it short-circuits to `Ok(true)` as designed.

See `rim_infra_services/src/capability.rs:197-207`.

---

## 5. Outcome surface

All audit entries carry one of seven typed outcome values. The type is defined
in `rim_services/src/audit/dto.rs:18-28`. Writers must use the typed enum — not
string literals. Conversion to/from the DB `text` column happens at the repo
boundary via `AuditOutcome::as_str()` and `AuditOutcome::try_from(&str)`.

| Rust variant | DB string | GraphQL enum value | Meaning |
|---|---|---|---|
| `Success` | `success` | `SUCCESS` | Command completed without error |
| `Error` | `error` | `ERROR` | Command terminated with an unhandled error |
| `ConfirmationRequired` | `confirmation_required` | `CONFIRMATION_REQUIRED` | Execution halted; user confirmation was requested |
| `PermissionDenied` | `permission_denied` | `PERMISSION_DENIED` | Capability or membership check denied the command |
| `RateLimited` | `rate_limited` | `RATE_LIMITED` | Per-actor rate limit exceeded |
| `StepError` | `step_error` | `STEP_ERROR` | One step inside a macro returned an error |
| `MacroPartialFailure` | `macro_partial_failure` | `MACRO_PARTIAL_FAILURE` | Macro completed partial steps before one failed |

GraphQL type: `AuditOutcomeGql` in `rim_api/src/types.rs:2550-2558`.
The DB column uses a CHECK constraint added in migration
`m20260427_…_audit_outcome_widen_check`.

`StepError` and `MacroPartialFailure` are set on child rows emitted for macro
steps. `parent_audit_id` links a child row back to the top-level macro entry.
`parent_audit_id=None` means the row is a top-level (non-macro-step) entry.

---

## 6. Closed bypasses (do not reopen)

### #23 P6-H2 — `server_id=None` bypass

**Vector:** Caller supplies `actor_user_id=<victim>` and omits `server_id`.
The original code forwarded the query to the repo without requiring workspace
context, allowing cross-user reads with no membership check.

**Fix:** Service-layer match on `(Some(other), None)` arm returns
`PermissionDenied` before any capability check or DB call.

**Location:** `rim_infra_services/src/audit/mod.rs:129-131`

**Regression test:** `p6_h2_cross_user_no_server_forbidden` in
`rim_infra_services/src/audit/mod.rs` (unit, mockall, no DB required).

---

### #31 P6-FU6 — Cross-user enumeration primitive

**Vector:** Caller with `audit.read` in workspace W supplies
`actor_user_id=<victim>, server_id=W`. The original code forwarded the
`actor_user_id` filter to the DB, returning only the victim's rows. This let an
authorized admin determine whether a specific user ever acted in W (presence
enumeration) and, if so, read their complete activity timeline.

**Fix:** When the `(Some(other), Some(W))` branch fires, `strip_actor_filter`
is set to `true`. The repo query is constructed with `actor_user_id=None`,
returning the full workspace dump regardless of the requested target user.

**Location:** `rim_infra_services/src/audit/mod.rs:88-126` (flag set),
`rim_infra_services/src/audit/mod.rs:146-159` (flag applied).

**Regression tests:**
- `p6_fu6_cross_user_workspace_strips_actor_filter` — repo receives `actor_user_id=None`
- `p6_fu6_cross_user_workspace_returns_full_workspace_dump` — response includes rows from all users
- `p6_fu6_cross_user_workspace_no_victim_rows_returns_workspace_dump_not_empty` — empty victim timeline does not produce empty response

All three are unit tests in `rim_infra_services/src/audit/mod.rs`.

---

### #30 P6-FU5 — `capability_check::membership_holds(None)` silent bypass

**Vector:** A workspace-scoped capability (e.g. `audit.read`) called with
`workspace_id=None` reached `membership_holds(actor, None)`, which returned
`Ok(true)` for any actor. The membership leg was silently skipped, granting the
capability without any workspace context.

**Fix:** Step 2b in `CapabilityService::check` tests `workspace_id.is_none() &&
!reg.is_self_scoped` and returns `PermissionDenied` before `membership_holds` is
called. Self-scoped capabilities (`is_self_scoped=true`, e.g. `audit.read.self`)
are exempt from this requirement by design.

**Location:** `rim_infra_services/src/capability.rs:197-207`

**Regression tests:**
- `check_workspace_scoped_cap_without_workspace_denies`
- `check_self_scoped_cap_without_workspace_succeeds`
- `check_many_workspace_scoped_cap_without_workspace_returns_false`

All three are unit tests in `rim_infra_services/src/capability.rs`.

---

## 7. Test fixture locations

### Unit tests (no DB required)

| File | Coverage |
|---|---|
| `rim_infra_services/src/audit/mod.rs` | Full access-control matrix (#23, #31); P6-H2 and P6-FU6 regression cases |
| `rim_infra_services/src/capability.rs` | Capability gate (#30); strict default-deny (C1); P6-FU5 regression cases |

### GraphQL layer tests

| File | Coverage |
|---|---|
| `rim_api/src/tests.rs:2664` (`list_audit_log_query_requires_auth`) | Unauthenticated `listAuditLog` must return errors |
| `rim_api/src/tests.rs:2679` (`list_audit_log_query_returns_page`) | Authenticated query returns correct shape; `AuditOutcome::Success` maps to `"SUCCESS"` in GraphQL |

### DB-backed integration tests (require `DATABASE_URL`, run with `--features db-tests`)

| File | Coverage |
|---|---|
| `rim_infra_db/tests/audit_log_repo.rs` | Repo-level insert and list; all 7 `AuditOutcome` variants written and read back |

**Drift-guard anchor for ticket #33:** any future test that asserts "the matrix
in this document is enforced end-to-end" should be placed in
`rim_infra_services/src/audit/mod.rs` (service-layer gate) and
`rim_api/src/tests.rs` (GraphQL surface). The DB repo tests in
`rim_infra_db/tests/audit_log_repo.rs` cover the persistence contract, not the
access-control matrix.

---

## 8. Retention

Audit rows are subject to automatic purge. The retention task runs hourly.

| Setting | Value |
|---|---|
| Default retention | 90 days |
| Maximum (operator-configurable) | 365 days |
| Env var override | `RIM_AUDIT_RETENTION_DAYS` |

Source: `rim_infra_services/src/audit/retention.rs:19-25`.
Values agreed at Kuba's Gate-1 review (2026-04-23, `ai-reframe-review-2026-04-22.md` §7.4 L3).
