# Audit Log Access Matrix

This document is the reference for which actor can read which rows of the command audit log
(`command_audit_log`, GraphQL `listAuditLog`) under which conditions. Anyone changing
`list_entries` in `rim_infra_services/src/audit/mod.rs` or `check` in
`rim_infra_services/src/capability.rs` reads this first. Section 6 lists the bypasses that were
closed, so they are not reintroduced.

---

## 1. Actors

The audit-read gate is capability-based: it asks whether the caller holds `audit.read` or
`audit.read.self`. A capability check is a registry lookup plus, for a workspace-scoped
capability, workspace membership. `audit.read` also requires a role: only the Owners and
Moderators of a workspace hold it there. Members and guests do not; like everyone else they read
only their own rows.

| Actor type | Capability that passes | Notes |
|---|---|---|
| Owner or Moderator of workspace W | `audit.read` in W, `audit.read.self` | Full dump of W's rows |
| Member or Guest of workspace W | `audit.read.self` | Own rows only; `audit.read` in W is `PermissionDenied` |
| Any authenticated user | `audit.read.self` | Own rows only, no workspace needed |
| Server-internal code | none: `write_entry` has no capability check | Only server code writes rows (see section 3) |

Only the builtin capabilities are in the registry; nothing registers plugin capabilities on the
server.

---

## 2. Resource scopes

`list_entries` branches on two optional filter fields supplied by the caller: `actor_user_id`
and `server_id`. Their combination decides the scope:

| Scope | `actor_user_id` | `server_id` | Result |
|---|---|---|---|
| Self-read | `Some(caller)` | `None` or `Some(W)` | The caller's own rows |
| Workspace read | `None` | `Some(W)` | Every row of W, for an Owner or Moderator of W |
| Cross-user read | `Some(other)` | `Some(W)` | Every row of W, for an Owner or Moderator of W: the user filter is stripped |
| Cross-user unscoped | `Some(other)` | `None` | Always `PermissionDenied` |
| Unscoped | `None` | `None` | Always `PermissionDenied` |

A Member or Guest of W gets `PermissionDenied` on the workspace and cross-user reads. In a
cross-user read the caller may read W, but the `actor_user_id` filter is removed from the
database query. The response is the full workspace dump, not one user's timeline, so a caller
cannot learn whether a given user ever acted in W from an empty or non-empty answer. Section 4.2
has the details.

---

## 3. Required capabilities

Two capabilities gate audit reads. Both are seeded at startup from `builtin_capabilities()` in
`rim_infra_services/src/capability.rs`.

| Capability | `is_self_scoped` | `requires_moderator` | Required for |
|---|---|---|---|
| `audit.read` | `false` (workspace-scoped) | `true` (Owner or Moderator) | Workspace read, cross-user read |
| `audit.read.self` | `true` (self-scoped) | `false` | Self-read, no workspace membership needed |

`is_self_scoped` is read from the `capability_registry` row, not hardcoded in the check. A
workspace-scoped capability checked without a `workspace_id` is `PermissionDenied` before the
workspace leg runs, so passing `None` cannot skip membership.

`requires_moderator` is not in the registry table. It lives only on `BuiltinCapabilitySpec`, and
the check reads it from `builtin_capabilities()` by key. `audit.read` is the only builtin with it
set; every other workspace-scoped capability needs membership alone.

**`DefaultCapabilityService::check`:**

```
step 1: empty or whitespace key → Validation
step 2: registry lookup; unknown or retired key → PermissionDenied (strict default-deny)
step 2b: workspace-scoped capability with workspace_id = None → PermissionDenied
step 3: workspace_id = None (self-scoped only) → allowed
        not a member of workspace_id → PermissionDenied
        requires_moderator and role is Member or Guest → PermissionDenied
```

`check_many` looks up the caller's role once and applies the same rules to every key.

**Write path.** `write_entry` has no capability check. Only server-internal code calls it: the
command executor, the macro executor, the moderation audit helper and the remote-workspace host.
It is never exposed to clients.

---

## 4. Filters enforced at the service layer

The resolver (`list_audit_log` in `rim_api/src/query/audit.rs`) passes the client's filter
values through unchanged. The service is the only trust boundary.

### 4.1 Workspace scope is required for non-self reads

`list_entries` matches on `(actor_user_id, server_id)` together:

- `actor_user_id = Some(other)` with `server_id = None` is `PermissionDenied` before any
  capability check or database call.
- Every non-self path needs a `server_id`; the `audit.read` check passes
  `workspace_id = Some(W)` to `CapabilityService::check`, which runs the workspace leg and
  requires an Owner or Moderator of W.

### 4.2 The cross-user filter is stripped

When the caller sends `actor_user_id = Some(other)` with `server_id = Some(W)`, the service:

1. checks `audit.read` in W (Owner or Moderator of W);
2. logs a `cross_user_filter_stripped` warning on the `audit` target;
3. sets `actor_user_id = None` in the repository query.

The database returns every row of W, so the response cannot reveal whether a specific user ever
acted there.

### 4.3 No membership bypass through a missing workspace

Step 3 of `check` allows a check with no workspace without looking at membership. Step 2b makes
sure only self-scoped capabilities reach it with `None`: a workspace-scoped capability without a
workspace is refused first.

---

## 5. Outcome values

Every audit row carries one of seven outcomes, the `AuditOutcome` enum in
`rim_services/src/audit/dto.rs`. Writers use the enum, never string literals; the conversion to
the `text` column happens at the repository boundary (`AuditOutcome::as_str`,
`TryFrom<&str>`).

| Rust variant | DB string | GraphQL value | Meaning |
|---|---|---|---|
| `Success` | `success` | `SUCCESS` | Command completed without error |
| `Error` | `error` | `ERROR` | Command ended with an error |
| `ConfirmationRequired` | `confirmation_required` | `CONFIRMATION_REQUIRED` | Execution stopped; a confirmation was requested |
| `PermissionDenied` | `permission_denied` | `PERMISSION_DENIED` | Capability or membership check refused the command |
| `RateLimited` | `rate_limited` | `RATE_LIMITED` | Per-actor rate limit exceeded |
| `StepError` | `step_error` | `STEP_ERROR` | One step inside a macro returned an error |
| `MacroPartialFailure` | `macro_partial_failure` | `MACRO_PARTIAL_FAILURE` | A macro ran some steps before one failed |

The GraphQL type is `AuditOutcomeGql` in `rim_api/src/types/audit.rs`. The database column has a
CHECK constraint from migration `m20260427_000001_audit_outcome_widen_check`.

`StepError` and `MacroPartialFailure` appear on the child rows written for macro steps.
`parent_audit_id` links a child row to the top-level macro row; `None` marks a top-level row.

---

## 6. Closed bypasses (do not reopen)

### Cross-user read without a workspace

**Vector:** the caller sends `actor_user_id = <victim>` and omits `server_id`. The query reached
the repository with no workspace context, so anyone could read another user's history.

**Fix:** the `(Some(other), None)` arm of `list_entries` returns `PermissionDenied` before any
capability check or database call.

### Cross-user enumeration

**Vector:** a caller with `audit.read` in W sends `actor_user_id = <victim>, server_id = W`. The
filter reached the database, so the answer told the caller whether the victim ever acted in W
and returned their whole timeline there.

**Fix:** the `(Some(other), Some(W))` arm sets `strip_actor_filter`, and the repository query is
built with `actor_user_id = None`, returning the full workspace dump.

### Membership skipped for a capability without a workspace

**Vector:** a workspace-scoped capability (for example `audit.read`) checked with
`workspace_id = None` reached the membership leg, which answered "allowed" for any actor when
there was no workspace.

**Fix:** step 2b of `check` refuses `workspace_id.is_none() && !reg.is_self_scoped` before the
workspace leg runs. Self-scoped capabilities such as `audit.read.self` are exempt by design.

### Every member read the whole workspace log

**Vector:** `audit.read` was granted on membership alone, so any member of W, guests included,
could read every row of W, including other members' commands and moderation actions.

**Fix:** `audit.read` carries `requires_moderator`, and step 3 of `check` denies it to a Member or
Guest of W. They keep `audit.read.self` for their own rows.

---

## 7. Where the tests are

| File | Coverage |
|---|---|
| `rim_infra_services/src/audit/mod.rs` | Unit tests of the whole matrix in section 2, including the regressions for the first two bypasses of section 6, and the role outcome of every arm through the real capability gate |
| `rim_infra_services/src/capability.rs` | Unit tests of the capability gate: strict default-deny, `check` / `check_many` with a workspace-scoped capability and no workspace (the third bypass), and the Owner/Moderator requirement of `audit.read` (the fourth) |
| `rim_api/src/tests/audit.rs` | `listAuditLog` refuses an unauthenticated caller; an authenticated query returns a page and maps `AuditOutcome::Success` to `SUCCESS` |
| `rim_api/tests/moderation_e2e.rs` | `listAuditLog` through GraphQL and Postgres under `--features e2e-tests`: Owners and Moderators read the workspace log, Members and Guests are `FORBIDDEN` and still read their own rows |
| `rim_infra_db/tests/audit_log_repo.rs` | Repository insert and list under `--features db-tests`; all seven outcomes written and read back |

A test asserting that this matrix holds end to end belongs in `rim_infra_services/src/audit/mod.rs`
(the service gate) and `rim_api/src/tests/audit.rs` (the GraphQL surface). The repository tests
cover persistence, not access control.

---

## 8. Retention

Audit rows are purged only when the operator sets a retention. A purge runs a minute after boot
and then daily; it exports the rows to the data directory before deleting them.

| Setting | Value |
|---|---|
| Default retention | none: rows are kept forever |
| Range (operator-configurable) | 1 to 3650 days |
| Env var | `RIM_AUDIT_RETENTION_DAYS` |

Source: `rim_infra_services/src/audit_retention.rs`. Operator view, the export format and the
`audit purge` command: `docs/operations/audit-retention.md`.
