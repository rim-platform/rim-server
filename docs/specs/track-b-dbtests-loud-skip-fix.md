# Track B — Fix silent db-test skip (P6-W6 loud-failure contract)

**Author:** iron-man · **Implementer:** hulk · **Requested by:** nick-fury
**Priority:** do this **first** — B1/B2/B3 all add db-tests and rely on this contract holding.
**Type:** test-harness bug fix. No production code, no schema.

## Problem
`db_conn_or_skip()` returns `None` (after an `eprintln!("SKIP: …")`) when `DATABASE_URL` is unset
or empty, and every caller uses `let Some(..) = … else { return; }`, so each test becomes a no-op
the harness counts as **passed**. Running `cargo test -p rim_infra_db --features db-tests` with no
DB reports a green "0 real tests ran". This contradicts CLAUDE.md's stated P6-W6 contract ("these
tests PANIC if DATABASE_URL is unset — intentional").

The separate `*_guard.rs` files (`db_tests_guard.rs`, `infra_db_tests_guard.rs`,
`e2e_tests_guard.rs`) do panic — but each lives in its **own** integration-test binary, so they
only fail the aggregate run. A filtered run (`--test user_repo`) or reading per-binary output still
shows a misleading green. The loud failure must live in the helper every test actually calls.

## Three identical copies (fix all three)
| Crate | Feature | Helper (`tests/support/mod.rs`) | Guard file |
|---|---|---|---|
| `rim_infra_db` | `db-tests` | `db_conn_or_skip` @ `mod.rs:57` | `tests/db_tests_guard.rs` |
| `rim_infra_services` | `infra-db-tests` | `db_conn_or_skip` @ `mod.rs:40` | `tests/infra_db_tests_guard.rs` |
| `rim_api` | `e2e-tests` | `db_conn_or_skip` @ `mod.rs:50` | `tests/e2e_tests_guard.rs` |

## Fix

**Rename `db_conn_or_skip` → `db_conn` and make it panic; return the connection directly (no
`Option`).** The rename is deliberate: it is compiler-enforced (no caller can be missed) and the
name no longer implies a skip. Because every `tests/` file is gated by a crate-level
`#![cfg(feature = "…")]`, the helper only compiles/runs when the feature is on, so panicking there
is exactly the P6-W6 contract and closes the filtered-run hole.

New helper body (adapt the eprintln/crate name per copy):
```rust
/// Connect to the test database, or **panic loudly**. Only reachable under the
/// crate's db-test feature (all test files are `#![cfg(feature = "…")]`-gated),
/// so a panic here means the feature was enabled without a usable test DB —
/// which must fail CI, not silently skip. See CLAUDE.md P6-W6.
pub async fn db_conn() -> DatabaseConnection {
    let url = match env::var("DATABASE_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "DATABASE_URL must be set when running with --features db-tests \
             (e.g. postgres://rim:rim@localhost/rim_testdb). Every db-test requires it; \
             without it the suite would silently skip and report a false green."
        ),
    };
    assert!(
        is_test_db_url(&url),
        "DATABASE_URL must point to a test DB (dbname containing 'testdb'/'_test'/'testing'); \
         refusing to run destructive db-tests against: {}",
        redact_url(&url),
    );

    let mut opts = ConnectOptions::new(url.clone());
    opts.sqlx_logging(false);
    let conn = Database::connect(opts)
        .await
        .expect("failed to connect to DATABASE_URL");
    eprintln!("db-tests: connected to {}", redact_url(&url));

    if MIGRATED.get().is_none() {
        migration::Migrator::up(&conn, None)
            .await
            .expect("failed to run migrations");
        let _ = MIGRATED.set(());
    }
    conn
}
```
Note the second change vs today: a **set-but-non-test** URL now `assert!`-panics instead of
returning `None` (it was a second silent-skip hole). This matches the guard's intent and prevents
running destructive resets against a non-test DB.

### Caller updates (compiler-guarded by the rename)
- `TestCtx::new` (`rim_infra_db/tests/support/mod.rs:150`, `rim_api/tests/support/mod.rs:129`):
  change return type `Option<Self>` → `Self`; body becomes
  `let conn = db_conn().await; reset_db(&conn).await; Self { db: conn.into() }`.
- Every test using `let Some(ctx) = TestCtx::new().await else { return; };` → `let ctx =
  TestCtx::new().await;`. Affected: ~54 tests across 15 `rim_infra_db` files and 5 in
  `rim_api/tests/registration_resolvers_e2e.rs`.
- Every direct `let Some(db) = support::db_conn_or_skip().await else { return; };` →
  `let db = support::db_conn().await;`. Affected: `rim_infra_services/tests/signup_txn_regression.rs`
  lines 82, 161, 241, 326, 378, 423, 486, 557 (8 sites).
- `rim_infra_services/tests/command_palette_workspace_fallback_test.rs` already reads
  `DATABASE_URL` directly and fails loudly — leave it, but if trivial, switch it to `db_conn()` for
  consistency (optional).

### Guard files
Keep the three `*_guard.rs` files. They still add value as a **fast fail-fast** that panics without
touching a DB (before the slower connect attempt). No changes needed; they are now belt-and-suspenders.

## Tests / acceptance
- With `DATABASE_URL` unset: `cargo test -p rim_infra_db --features db-tests -- --test-threads=1`
  must **fail** (non-zero exit, visible panic), not report green. Same for the other two crates/features.
- With a valid test `DATABASE_URL`: full db-test suites pass exactly as before (behavior unchanged
  on the happy path — same connect, same migrate-once via `MIGRATED`, same `reset_db`).
- `cargo test --workspace` (no db features) is unaffected — the helper and its callers don't compile.
- Add one tiny positive test is unnecessary; the guard files already assert the panic path via
  `catch_unwind`. Leave them.

## Docs
Update the `db_conn` doc comment (above). Optionally add a one-line note to CLAUDE.md's db-test
section that the loud failure now lives in `db_conn` itself (the helper), not only the guard
binaries — but that is a doc nicety, not required.

## Rollback
Pure test-harness change; revert is a code revert. No schema, no runtime impact.
