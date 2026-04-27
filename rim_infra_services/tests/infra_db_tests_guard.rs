#![cfg(feature = "infra-db-tests")]

/// Fail loudly when `--features infra-db-tests` is active but `DATABASE_URL`
/// is not set. Without this guard the integration tests silently skip
/// themselves (via `db_conn_or_skip()` returning `None`) and the CI job
/// reports a false green — every test "passes" because none actually ran.
///
/// This test is synchronous and has no DB dependency; it exists purely to
/// convert the silent skip into a hard failure so misconfigured CI is
/// immediately visible.
#[test]
fn infra_db_tests_require_database_url() {
    let url = std::env::var("DATABASE_URL").unwrap_or_default();
    assert!(
        !url.trim().is_empty(),
        "DATABASE_URL must be set when running with --features infra-db-tests. \
         Set DATABASE_URL to a postgres:// test database URL (e.g. \
         'postgres://rim:rim@localhost/rim_testdb') and re-run. \
         Without it every infra-db-test silently skips and CI reports a false green."
    );
}

/// Verify the guard itself panics when DATABASE_URL is absent.
/// Uses `std::panic::catch_unwind` after temporarily removing the env var
/// so the assertion can be tested without actually misconfiguring CI.
///
/// Requires `--test-threads=1` because it mutates the process environment.
#[test]
fn infra_db_tests_guard_panics_when_url_absent() {
    let saved = std::env::var("DATABASE_URL").ok();

    // SAFETY: single-threaded test runner required (--test-threads=1 in CI).
    // Environment mutation is visible to all threads in the process.
    unsafe { std::env::remove_var("DATABASE_URL") };

    let result = std::panic::catch_unwind(|| {
        let url = std::env::var("DATABASE_URL").unwrap_or_default();
        assert!(
            !url.trim().is_empty(),
            "DATABASE_URL must be set when running with --features infra-db-tests."
        );
    });

    // Restore before asserting so a test failure here doesn't leave env dirty.
    match saved {
        Some(v) => unsafe { std::env::set_var("DATABASE_URL", &v) },
        None => unsafe { std::env::remove_var("DATABASE_URL") },
    }

    assert!(
        result.is_err(),
        "guard must panic when DATABASE_URL is absent"
    );
}
